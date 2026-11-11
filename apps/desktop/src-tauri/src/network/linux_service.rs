//! Linux TUN service client. A live Unix connection owns one service session;
//! closing the GUI also closes that connection, allowing the service to recover.

use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use socket2::{Domain, SockAddr, Socket, Type};

const SERVICE: &str = "/usr/lib/crossbow/crossbow-service";
const CORE: &str = "/usr/lib/crossbow/mihomo";
const SOCKET: &str = "/run/crossbow-tun/control.sock";
const UNIT: &str = "crossbow-tun.service";
const SYSTEMCTL: &str = "/usr/bin/systemctl";
const PKEXEC: &str = "/usr/bin/pkexec";
const MAX_FRAME: usize = 4096;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const START_TIMEOUT: Duration = Duration::from_secs(35);
const STOP_TIMEOUT: Duration = Duration::from_secs(12);

static CLIENT: Mutex<ServiceClient> = Mutex::new(ServiceClient {
    session: None,
    terminal_failure: None,
});

struct ServiceClient {
    session: Option<Session>,
    terminal_failure: Option<(u32, String)>,
}

#[derive(Debug, Clone, Copy)]
pub struct ServiceStatus {
    pub pid: u32,
    pub ifindex: u32,
    pub active: bool,
}

struct Session {
    stream: UnixStream,
    service_pid: u32,
    core: Option<ServiceStatus>,
    wireguard_coexist: bool,
    broken: bool,
}

pub fn binary_path() -> PathBuf {
    PathBuf::from(CORE)
}

fn protected_owner_mode(uid: u32, mode: u32) -> bool {
    uid == 0 && mode & 0o022 == 0
}

/// Reject writable ancestors and symlinks as well as a writable executable.
/// Both peers rely on an administrator-installed binary, never a workspace copy.
fn protected_executable(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("服务程序必须使用固定绝对路径".into());
    }
    for (index, component) in path.ancestors().enumerate() {
        let metadata = fs::symlink_metadata(component)
            .map_err(|_| format!("缺少受保护的服务文件：{}", component.display()))?;
        let correct_type = if index == 0 {
            metadata.is_file() && metadata.mode() & 0o111 != 0
        } else {
            metadata.is_dir()
        };
        if !correct_type || !protected_owner_mode(metadata.uid(), metadata.mode()) {
            return Err(format!(
                "服务文件及目录必须由 root 持有，且不可由组或其他用户写入：{}",
                component.display()
            ));
        }
    }
    Ok(())
}

fn validate_installation() -> Result<(), String> {
    protected_executable(Path::new(SERVICE))?;
    protected_executable(Path::new(CORE))
}

pub fn installed() -> bool {
    validate_installation().is_ok()
}

fn peer_credentials(stream: &UnixStream) -> Result<libc::ucred, String> {
    let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if result != 0 || len as usize != std::mem::size_of::<libc::ucred>() {
        return Err("无法验证 Linux 服务的连接身份".into());
    }
    if credentials.pid <= 0 || credentials.uid != unsafe { libc::getuid() } {
        return Err("Linux 服务不属于当前用户".into());
    }
    Ok(credentials)
}

fn unit_matches(text: &str, pid: u32, uid: u32) -> bool {
    let field = |name: &str| {
        let prefix = format!("{name}=");
        let mut matches = text.lines().filter_map(|line| line.strip_prefix(&prefix));
        let value = matches.next()?;
        matches.next().is_none().then_some(value)
    };
    let expected_exec = format!("{{ path={SERVICE} ; argv[]={SERVICE} --serve ; ");
    field("MainPID").and_then(|value| value.parse::<u32>().ok()) == Some(pid)
        && field("User").and_then(|value| value.parse::<u32>().ok()) == Some(uid)
        && field("LoadState") == Some("loaded")
        && field("Transient") == Some("yes")
        && field("ExecStart").is_some_and(|value| {
            value.starts_with(&expected_exec)
                && value.ends_with(" }")
                && value.matches("{ path=").count() == 1
        })
}

fn verify_peer(stream: &UnixStream) -> Result<u32, String> {
    let peer = peer_credentials(stream)?;
    match fs::read_link(format!("/proc/{}/exe", peer.pid)) {
        Ok(executable) if executable == Path::new(SERVICE) => Ok(peer.pid as u32),
        Ok(_) => Err("连接端点不是已安装的 Crossbow 服务".into()),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            // CAP_NET_ADMIN can make procfs identity checks inaccessible to the
            // unprivileged GUI. Ask the system manager which process it started;
            // a user-owned socket alone is never proof of service identity.
            protected_executable(Path::new(SYSTEMCTL))?;
            let mut command = Command::new(SYSTEMCTL);
            command.args([
                "--system",
                "show",
                UNIT,
                "--no-pager",
                "--property=MainPID,User,ExecStart,LoadState,Transient",
            ]);
            command.env("LC_ALL", "C");
            let output = command_output(command, REQUEST_TIMEOUT)?;
            if unit_matches(&output, peer.pid as u32, peer.uid) {
                Ok(peer.pid as u32)
            } else {
                Err("systemd 未确认此连接属于 Crossbow 服务".into())
            }
        }
        Err(_) => Err("无法读取 Linux 服务的程序身份".into()),
    }
}

fn connect() -> Result<Session, String> {
    validate_installation()?;
    let socket = Socket::new(Domain::UNIX, Type::STREAM, None)
        .map_err(|error| format!("创建服务连接失败：{error}"))?;
    let address = SockAddr::unix(SOCKET).map_err(|error| error.to_string())?;
    socket
        .connect_timeout(&address, Duration::from_secs(3))
        .map_err(|error| format!("Linux TUN 服务未就绪：{error}"))?;
    let stream = UnixStream::from(socket);
    let service_pid = verify_peer(&stream)?;
    Ok(Session {
        stream,
        service_pid,
        core: None,
        wireguard_coexist: false,
        broken: false,
    })
}

fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "service request timed out"))
}

/// One bounded frame and one deadline, including a peer that sends slowly.
/// The second result records a disconnect even if a terminal error was readable.
fn exchange(
    stream: &mut UnixStream,
    payload: &Value,
    timeout: Duration,
) -> Result<(Value, bool), String> {
    let mut frame = serde_json::to_vec(payload).map_err(|error| error.to_string())?;
    if frame.len() >= MAX_FRAME {
        return Err("Linux 服务请求过长".into());
    }
    frame.push(b'\n');
    let deadline = Instant::now() + timeout;
    let write = (|| -> io::Result<()> {
        let mut written = 0;
        while written < frame.len() {
            stream.set_write_timeout(Some(remaining(deadline)?))?;
            let count = stream.write(&frame[written..])?;
            if count == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            written += count;
        }
        Ok(())
    })();
    let write_error = match write {
        Ok(()) => None,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
            ) =>
        {
            Some(error)
        }
        Err(error) => return Err(format!("Linux 服务通信失败：{error}")),
    };
    let response = read_response(stream, deadline);
    if let Some(error) = write_error {
        // The authenticated service may have sent its failure before closing.
        // A buffered success cannot acknowledge a request we failed to send.
        return response
            .ok()
            .filter(terminal_failure_frame)
            .map(|response| (response, true))
            .ok_or_else(|| format!("Linux 服务通信失败：{error}"));
    }
    response.map(|response| (response, false))
}

fn read_response(stream: &mut UnixStream, deadline: Instant) -> Result<Value, String> {
    let mut read = || -> io::Result<Vec<u8>> {
        let mut response = Vec::new();
        loop {
            stream.set_read_timeout(Some(remaining(deadline)?))?;
            let mut byte = [0u8];
            stream.read_exact(&mut byte)?;
            if byte[0] == b'\n' {
                return Ok(response);
            }
            if response.len() >= MAX_FRAME {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "service response too large",
                ));
            }
            response.push(byte[0]);
        }
    };
    let response = read().map_err(|error| format!("Linux 服务通信失败：{error}"))?;
    let value: Value = serde_json::from_slice(&response).map_err(|_| "Linux 服务返回了无效响应")?;
    if value.get("ok").and_then(Value::as_bool).is_none() {
        return Err("Linux 服务响应缺少结果状态".into());
    }
    Ok(value)
}

impl Session {
    fn request(&mut self, payload: Value, timeout: Duration) -> Result<Value, String> {
        if self.broken {
            return Err("Linux 服务连接已断开，清理结果尚未确认".into());
        }
        let result = exchange(&mut self.stream, &payload, timeout);
        if !matches!(result, Ok((_, false))) {
            self.broken = true;
            let _ = self.stream.shutdown(std::net::Shutdown::Both);
        }
        result.map(|(response, _)| response)
    }

    fn cleanup_confirmed(&self) -> Result<bool, String> {
        // ESRCH is the only accepted evidence of death here. EPERM and PID
        // reuse retain the session; no process receives a termination signal.
        if unsafe { libc::kill(self.service_pid as i32, 0) } == 0
            || io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
        {
            return Ok(false);
        }
        let program = ["/usr/sbin/ip", "/usr/bin/ip", "/sbin/ip"]
            .into_iter()
            .find_map(|path| fs::canonicalize(path).ok())
            .ok_or("需要 iproute2 才能确认网络恢复")?;
        protected_executable(&program)?;
        let read = |arguments: &[&str]| -> Result<Vec<Value>, String> {
            let mut command = Command::new(&program);
            command.args(arguments).env("LC_ALL", "C");
            serde_json::from_str(&command_output(command, REQUEST_TIMEOUT)?)
                .map_err(|_| "无法确认 Linux 网络资源清理结果".into())
        };
        let links = read(&["-j", "link", "show"])?;
        let mut rules = read(&["-N", "-j", "-4", "rule", "show"])?;
        rules.extend(read(&["-N", "-j", "-6", "rule", "show"])?);
        let mut routes = read(&["-N", "-j", "-4", "route", "show", "table", "all"])?;
        routes.extend(read(&["-N", "-j", "-6", "route", "show", "table", "all"])?);
        Ok(resources_absent(
            &links,
            &rules,
            &routes,
            self.wireguard_coexist,
        ))
    }
}

fn resources_absent(
    links: &[Value],
    rules: &[Value],
    routes: &[Value],
    wireguard_coexist: bool,
) -> bool {
    use crossbow_service::linux_network::{
        DEVICE, PRIORITY, TABLE, WIREGUARD_PRIORITY, WIREGUARD_TABLE,
    };
    let number = |value: &Value, expected: u32| {
        value.as_u64() == Some(expected as u64)
            || value
                .as_str()
                .is_some_and(|value| value == expected.to_string())
    };
    let reserved_table = |value: &Value| {
        number(value, TABLE) || (wireguard_coexist && number(value, WIREGUARD_TABLE))
    };
    !links.iter().any(|link| link["ifname"] == DEVICE)
        && !rules.iter().any(|rule| {
            number(&rule["priority"], PRIORITY)
                || (wireguard_coexist && number(&rule["priority"], WIREGUARD_PRIORITY))
                || reserved_table(&rule["table"])
        })
        && !routes
            .iter()
            .any(|route| route["dev"] == DEVICE || reserved_table(&route["table"]))
}

fn response_error(response: &Value) -> String {
    response
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("Linux 服务未完成操作")
        .to_string()
}

fn confirmed_stop(response: &Value) -> bool {
    response["ok"].is_boolean()
        && response["stopped"] == true
        && matches!(response.get("active"), None | Some(Value::Bool(false)))
}

fn terminal_failure_frame(response: &Value) -> bool {
    response["ok"] == false
        && response["stopped"].is_boolean()
        && matches!(response.get("active"), None | Some(Value::Bool(false)))
        && response["error"]
            .as_str()
            .is_some_and(|error| !error.is_empty())
}

fn positive_u32(response: &Value, field: &str) -> Result<u32, String> {
    response
        .get(field)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value != 0)
        .ok_or_else(|| format!("Linux 服务响应缺少有效的 {field}"))
}

/// Only an explicit TUN enable action may invoke this authorization flow.
pub fn authorize(wireguard_coexist: bool) -> Result<(), String> {
    validate_installation()?;
    if managed_pid().is_some() && active() {
        return Ok(());
    }
    if let Ok((_, supported)) = probe_idle_service() {
        return require_wireguard_support(wireguard_coexist, supported);
    }
    protected_executable(Path::new(PKEXEC))?;
    let mut child = Command::new(PKEXEC)
        .args(["--disable-internal-agent", SERVICE, "--authorize"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("无法请求 Linux 服务授权：{error}"))?;
    let deadline = Instant::now() + Duration::from_secs(120);
    let result = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.to_string());
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Linux 服务授权超时，TUN 未开启".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
        }
    };
    if !result.success() {
        let mut details = String::new();
        if let Some(stderr) = child.stderr.take() {
            let _ = stderr.take(4096).read_to_string(&mut details);
        }
        return Err(format!(
            "Linux 服务授权或启动失败（退出码 {}），TUN 未开启：{}",
            result
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "signal".into()),
            details.trim()
        ));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match probe_idle_service() {
            Ok((_, supported)) => return require_wireguard_support(wireguard_coexist, supported),
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn probe_idle_service() -> Result<(u32, bool), String> {
    let mut session = connect()?;
    let response = session.request(json!({"cmd":"status"}), REQUEST_TIMEOUT)?;
    if response["ok"] == true && response["active"] == false && response["stopped"] == true {
        Ok((
            session.service_pid,
            response["wireguard_coexist_supported"] == true,
        ))
    } else {
        Err("Linux 服务尚未确认可供本次会话使用".into())
    }
}

fn require_wireguard_support(requested: bool, supported: bool) -> Result<(), String> {
    if requested && !supported {
        Err(
            "已安装的 Linux 网络服务版本不支持 WireGuard 共存，请从当前版本安装包更新网络服务"
                .into(),
        )
    } else {
        Ok(())
    }
}

pub fn make_core_spawner(
    wireguard_coexist: bool,
) -> Result<impl FnOnce(&Path, &Path) -> Result<u32, String> + Send, String> {
    validate_installation()?;
    Ok(move |config: &Path, log: &Path| {
        let path_text = |path: &Path| -> Result<String, String> {
            if !path.is_absolute() {
                return Err("服务配置及日志路径必须是绝对路径".into());
            }
            path.to_str()
                .map(str::to_string)
                .ok_or_else(|| "服务路径不是有效 UTF-8".into())
        };
        let mut payload =
            json!({"cmd":"start", "config":path_text(config)?, "log":path_text(log)?});
        if wireguard_coexist {
            payload["wireguard_coexist"] = json!(true);
        }
        let mut client = CLIENT.lock().unwrap_or_else(|error| error.into_inner());
        let guard = &mut client.session;
        if guard.is_some() {
            return Err("上次 Linux 服务会话尚未确认清理，请先停止后重试".into());
        }
        // An older service ignores extra start fields. Verify support before it
        // can start ordinary TUN, and only send the request to the probed peer.
        let verified_pid = if wireguard_coexist {
            let (pid, supported) = probe_idle_service()?;
            require_wireguard_support(true, supported)?;
            Some(pid)
        } else {
            None
        };
        let mut session = connect()?;
        if verified_pid.is_some_and(|pid| pid != session.service_pid) {
            return Err("Linux 服务身份在预检后发生变化，未开启 TUN".into());
        }
        session.wireguard_coexist = wireguard_coexist;
        *guard = Some(session);
        let response = guard.as_mut().unwrap().request(payload, START_TIMEOUT)?;
        if response["ok"] != true {
            if confirmed_stop(&response) {
                *guard = None;
            }
            return Err(response_error(&response));
        }
        if response["active"] != true {
            return Err("Linux 服务尚未确认网络接管".into());
        }
        let status = ServiceStatus {
            pid: positive_u32(&response, "pid")?,
            ifindex: positive_u32(&response, "ifindex")?,
            active: true,
        };
        guard.as_mut().unwrap().core = Some(status);
        client.terminal_failure = None;
        Ok(status.pid)
    })
}

/// A transport error remains an error, not an inactive or successfully cleaned session.
pub fn status() -> Result<Option<ServiceStatus>, String> {
    CLIENT
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .status()
}

impl ServiceClient {
    fn status(&mut self) -> Result<Option<ServiceStatus>, String> {
        let Some(session) = self.session.as_mut() else {
            return Ok(None);
        };
        let response = match session.request(json!({"cmd":"status"}), REQUEST_TIMEOUT) {
            Ok(response) => response,
            Err(error) => {
                if session.cleanup_confirmed()? {
                    self.session = None;
                    return Ok(None);
                }
                return Err(error);
            }
        };
        if terminal_failure_frame(&response) {
            if let Some(core) = session.core {
                // Status and tray reads can reach the socket before the watcher.
                self.terminal_failure = Some((core.pid, response_error(&response)));
            }
        }
        if confirmed_stop(&response) {
            self.session = None;
            return if response["ok"] == true {
                Ok(None)
            } else {
                Err(response_error(&response))
            };
        }
        if response["ok"] != true {
            return Err(response_error(&response));
        }
        let status = ServiceStatus {
            pid: positive_u32(&response, "pid")?,
            ifindex: positive_u32(&response, "ifindex")?,
            active: response
                .get("active")
                .and_then(Value::as_bool)
                .ok_or("Linux 服务响应缺少接管状态")?,
        };
        if session
            .core
            .is_some_and(|core| core.pid != status.pid || core.ifindex != status.ifindex)
        {
            return Err("Linux 服务会话身份发生变化，未确认原会话已清理".into());
        }
        session.core = Some(status);
        Ok(Some(status))
    }

    fn watch_status(&mut self, pid: u32) -> (Result<Option<ServiceStatus>, String>, bool) {
        if let Some(core) = self.session.as_ref().and_then(|session| session.core) {
            if core.pid != pid {
                return (Ok(Some(core)), false);
            }
        }
        let result = self.status();
        let failure = self
            .terminal_failure
            .take_if(|(core_pid, _)| *core_pid == pid);
        (
            failure.map_or(result, |(_, error)| Err(error)),
            self.session.is_none(),
        )
    }
}

pub fn watch_status(pid: u32) -> (Result<Option<ServiceStatus>, String>, bool) {
    CLIENT
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .watch_status(pid)
}

pub fn active() -> bool {
    status().is_ok_and(|status| status.is_some_and(|status| status.active))
}

pub fn managed_pid() -> Option<u32> {
    CLIENT
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .session
        .as_ref()
        .and_then(|session| session.core.map(|core| core.pid))
}

/// `false` means no client-owned service session; callers may stop a normal core.
/// Once a session exists, only a confirmed service acknowledgement releases it.
pub fn stop_core() -> Result<bool, String> {
    let mut client = CLIENT.lock().unwrap_or_else(|error| error.into_inner());
    let guard = &mut client.session;
    let Some(session) = guard.as_mut() else {
        return Ok(false);
    };
    if session.broken {
        if session.cleanup_confirmed()? {
            *guard = None;
            return Ok(true);
        }
        // Reconnecting never starts a core or asks for privilege. The helper
        // must verify/recover its journal before acknowledging an idle stop.
        let previous_core = session.core;
        let wireguard_coexist = session.wireguard_coexist;
        *session = connect()?;
        session.core = previous_core;
        session.wireguard_coexist = wireguard_coexist;
    }
    let response = match session.request(json!({"cmd":"stop"}), STOP_TIMEOUT) {
        Ok(response) => response,
        Err(error) => {
            if session.cleanup_confirmed()? {
                *guard = None;
                return Ok(true);
            }
            return Err(error);
        }
    };
    if confirmed_stop(&response) {
        *guard = None;
        return Ok(true);
    }
    if response["ok"] != true {
        return Err(response_error(&response));
    }
    Err("Linux 服务尚未确认网络资源已清理".into())
}

fn command_output(mut command: Command, timeout: Duration) -> Result<String, String> {
    let mut child = command
        // Identity checks must query the real system bus, never a caller's
        // SYSTEMD_BUS_ADDRESS or LD_PRELOAD override.
        .env_clear()
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("读取服务状态失败：{error}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if !status.success() => return Err("服务状态查询失败".into()),
            Ok(Some(_)) => break,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.to_string());
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("服务状态查询超时".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    let mut output = Vec::new();
    child
        .stdout
        .take()
        .ok_or("服务状态输出缺失")?
        .take(8193)
        .read_to_end(&mut output)
        .map_err(|error| error.to_string())?;
    if output.len() > 8192 {
        return Err("服务状态输出过长".into());
    }
    String::from_utf8(output).map_err(|_| "服务状态输出不是有效 UTF-8".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_files_require_root_and_no_shared_write_permission() {
        assert!(protected_owner_mode(0, 0o100755));
        for (uid, mode) in [(1000, 0o100755), (0, 0o100775), (0, 0o100757)] {
            assert!(!protected_owner_mode(uid, mode));
        }
    }

    #[test]
    fn systemd_identity_must_match_the_entire_command_and_owner() {
        let valid = format!("MainPID=123\nUser=1000\nLoadState=loaded\nTransient=yes\nExecStart={{ path={SERVICE} ; argv[]={SERVICE} --serve ; ignore_errors=no ; pid=123 ; }}\n");
        assert!(unit_matches(&valid, 123, 1000));
        for invalid in [
            valid.replace("User=1000", "User=0"),
            valid.replace("MainPID=123", "MainPID=124"),
            valid.replace("--serve ;", "--serve --arbitrary ;"),
            valid.replace("Transient=yes", "Transient=no"),
            format!("{valid}MainPID=123\n"),
            valid.replace(
                "/usr/lib/crossbow/crossbow-service",
                "/tmp/crossbow-service",
            ),
        ] {
            assert!(!unit_matches(&invalid, 123, 1000), "{invalid}");
        }
    }

    #[test]
    fn socket_frames_reject_incomplete_or_unbounded_responses() {
        for reply in [
            b"{\"ok\":true".to_vec(),
            b"{}\n".to_vec(),
            vec![b'x'; MAX_FRAME + 1],
        ] {
            let (mut client, mut server) = UnixStream::pair().unwrap();
            let worker = std::thread::spawn(move || {
                let mut request = [0u8; 128];
                let _ = server.read(&mut request);
                let _ = server.write_all(&reply);
            });
            assert!(exchange(
                &mut client,
                &json!({"cmd":"status"}),
                Duration::from_secs(1)
            )
            .is_err());
            worker.join().unwrap();
        }
    }

    #[test]
    fn same_user_socket_is_not_enough_to_identify_the_service() {
        let (client, _server) = UnixStream::pair().unwrap();
        assert!(peer_credentials(&client).is_ok());
        assert!(verify_peer(&client).is_err());
    }

    #[test]
    fn stalled_response_is_bounded_and_never_claims_success() {
        let (mut client, _server) = UnixStream::pair().unwrap();
        let started = Instant::now();
        assert!(exchange(
            &mut client,
            &json!({"cmd":"stop"}),
            Duration::from_millis(40)
        )
        .is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn closed_broker_preserves_terminal_failure_for_the_owning_watcher() {
        let (stream, mut broker) = UnixStream::pair().unwrap();
        let pid = 123;
        let mut client = ServiceClient {
            session: Some(Session {
                stream,
                service_pid: std::process::id(),
                core: Some(ServiceStatus {
                    pid,
                    ifindex: 7,
                    active: true,
                }),
                wireguard_coexist: true,
                broken: false,
            }),
            terminal_failure: None,
        };
        let reason = "WireGuard route changed";
        writeln!(
            broker,
            "{}",
            json!({"ok":false,"stopped":true,"error":reason})
        )
        .unwrap();
        drop(broker);

        // Simulate a settings or tray refresh reaching the closed socket first.
        assert_eq!(client.status().unwrap_err(), reason);
        assert!(client.status().unwrap().is_none());
        assert!(client.watch_status(pid + 1).0.unwrap().is_none());
        let (result, cleanup_confirmed) = client.watch_status(pid);
        assert_eq!(result.unwrap_err(), reason);
        assert!(cleanup_confirmed);
        assert!(client.watch_status(pid).0.unwrap().is_none());
    }

    #[test]
    fn closed_broker_does_not_turn_a_buffered_success_into_an_acknowledgement() {
        for reply in [
            json!({"ok":true,"stopped":true}),
            json!({"ok":false,"stopped":true,"active":true,"error":"inconsistent"}),
        ] {
            let (mut client, mut broker) = UnixStream::pair().unwrap();
            writeln!(broker, "{reply}").unwrap();
            drop(broker);
            assert!(exchange(&mut client, &json!({"cmd":"stop"}), REQUEST_TIMEOUT).is_err());
        }
    }

    #[test]
    fn closed_broker_preserves_error_without_confirming_failed_cleanup() {
        let (stream, mut broker) = UnixStream::pair().unwrap();
        let pid = 123;
        let mut client = ServiceClient {
            session: Some(Session {
                stream,
                service_pid: std::process::id(),
                core: Some(ServiceStatus {
                    pid,
                    ifindex: 7,
                    active: true,
                }),
                wireguard_coexist: true,
                broken: false,
            }),
            terminal_failure: None,
        };
        let reason = "TUN route cleanup failed";
        writeln!(
            broker,
            "{}",
            json!({"ok":false,"stopped":false,"error":reason})
        )
        .unwrap();
        drop(broker);

        let (result, cleanup_confirmed) = client.watch_status(pid);
        assert_eq!(result.unwrap_err(), reason);
        assert!(!cleanup_confirmed);
        assert!(client
            .session
            .as_ref()
            .is_some_and(|session| session.broken));
    }

    #[test]
    fn cleanup_requires_both_route_families_and_device_to_be_empty() {
        use crossbow_service::linux_network::{DEVICE, PRIORITY, TABLE};
        assert!(resources_absent(&[], &[], &[], false));
        assert!(!resources_absent(
            &[json!({"ifname": DEVICE})],
            &[],
            &[],
            false
        ));
        assert!(!resources_absent(
            &[],
            &[json!({"priority": PRIORITY})],
            &[],
            false
        ));
        assert!(!resources_absent(
            &[],
            &[json!({"table": TABLE.to_string()})],
            &[],
            false
        ));
        assert!(!resources_absent(
            &[],
            &[],
            &[json!({"table": TABLE})],
            false
        ));
        assert!(!resources_absent(
            &[],
            &[],
            &[json!({"dev": DEVICE})],
            false
        ));
        assert!(resources_absent(
            &[json!({"ifname":"wg-work"})],
            &[json!({"priority":5270,"table":52})],
            &[json!({"table":52,"dev":"tailscale0"})],
            false
        ));
    }

    #[test]
    fn coexist_cleanup_requires_the_wireguard_resources_to_be_absent() {
        use crossbow_service::linux_network::{WIREGUARD_PRIORITY, WIREGUARD_TABLE};
        let rules = [json!({"priority": WIREGUARD_PRIORITY})];
        let routes = [json!({"table": WIREGUARD_TABLE, "dev": "wg-work"})];
        assert!(resources_absent(&[], &rules, &routes, false));
        assert!(!resources_absent(&[], &rules, &[], true));
        assert!(!resources_absent(&[], &[], &routes, true));
        assert!(resources_absent(&[], &[], &[], true));
    }

    #[test]
    fn terminal_failure_can_confirm_cleanup_but_an_active_or_incomplete_reply_cannot() {
        let failure = json!({"ok":false,"stopped":true,"error":"WireGuard route changed"});
        assert!(confirmed_stop(&failure));
        assert_eq!(response_error(&failure), "WireGuard route changed");
        assert!(confirmed_stop(&json!({"ok":true,"stopped":true})));
        assert!(!confirmed_stop(&json!({"ok":false,"stopped":false})));
        assert!(!confirmed_stop(
            &json!({"ok":false,"stopped":true,"active":true})
        ));
        assert!(!confirmed_stop(
            &json!({"ok":false,"stopped":true,"active":"unknown"})
        ));
        assert!(!confirmed_stop(&json!({"stopped":true})));
    }
}
