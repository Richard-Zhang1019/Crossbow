use std::ffi::{c_void, OsStr, OsString};
use std::io::Read;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::Authorization::*;
use windows_sys::Win32::Security::*;
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::Pipes::*;
use windows_sys::Win32::System::Services::*;
use windows_sys::Win32::System::Threading::*;

const PIPE: &str = "\\\\.\\pipe\\crossbow-service";
static STOP: AtomicBool = AtomicBool::new(false);
static STATUS: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}
fn text(s: &str) -> Vec<u16> {
    wide(OsStr::new(s))
}
fn error() -> String {
    std::io::Error::last_os_error().to_string()
}
struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
struct Local(*mut c_void);
impl Drop for Local {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
struct Impersonation;
impl Impersonation {
    fn enter(pipe: HANDLE) -> Result<Self, String> {
        if unsafe { ImpersonateNamedPipeClient(pipe) } == 0 {
            return Err(error());
        }
        Ok(Self)
    }
}
impl Drop for Impersonation {
    fn drop(&mut self) {
        // Continuing as SYSTEM after a failed revert is unsafe.
        if unsafe { RevertToSelf() } == 0 {
            std::process::abort();
        }
    }
}

fn report(state: u32) {
    let handle = STATUS.load(Ordering::SeqCst);
    if handle.is_null() {
        return;
    }
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: if state == SERVICE_RUNNING {
            SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
        } else {
            0
        },
        dwWin32ExitCode: 0,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: if state == SERVICE_STOP_PENDING {
            5000
        } else {
            0
        },
    };
    unsafe {
        SetServiceStatus(handle, &status);
    }
}
unsafe extern "system" fn handler(control: u32) {
    if control == SERVICE_CONTROL_STOP || control == SERVICE_CONTROL_SHUTDOWN {
        STOP.store(true, Ordering::SeqCst);
        report(SERVICE_STOP_PENDING);
    }
}
unsafe extern "system" fn service_main(_argc: u32, _argv: *mut *mut u16) {
    let handle = RegisterServiceCtrlHandlerW(text("CrossbowService").as_ptr(), Some(handler));
    if handle.is_null() {
        return;
    }
    STATUS.store(handle, Ordering::SeqCst);
    report(SERVICE_START_PENDING);
    if let Ok(mut broker) = Broker::new() {
        report(SERVICE_RUNNING);
        // The SCM entry point must stay alive until STOP; no detached server thread.
        broker.serve();
        broker.stop();
    }
    report(SERVICE_STOPPED);
}

/// 服务目录可能位于用户自选盘符（非管理员可写）：启动即收紧服务与
/// 托管内核二进制的 DACL——非管理员只读可执行，封死换文件提权。
/// SYSTEM/管理员全权不受影响，安装器（提权）升级不受影响。
fn restrict_write_acl(path: &Path) {
    let mut descriptor = std::ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;RX;;;BU)").as_ptr(),
            1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return;
    }
    let _descriptor = Local(descriptor);
    unsafe {
        SetFileSecurityW(
            wide(path.as_os_str()).as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        );
    }
}

fn no_reparse(path: &Path) -> Result<(), String> {
    for part in path.ancestors() {
        if std::fs::symlink_metadata(part)
            .map_err(|e| e.to_string())?
            .file_attributes()
            & FILE_ATTRIBUTE_REPARSE_POINT
            != 0
        {
            return Err("reparse points are not allowed in service paths".into());
        }
    }
    Ok(())
}
fn process_path(pid: u32) -> Result<(Handle, PathBuf), String> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return Err(error());
    }
    let handle = Handle(handle);
    let mut buffer = vec![0u16; 32768];
    let mut len = buffer.len() as u32;
    if unsafe { QueryFullProcessImageNameW(handle.0, 0, buffer.as_mut_ptr(), &mut len) } == 0 {
        return Err(error());
    }
    let path = PathBuf::from(OsString::from_wide(&buffer[..len as usize]));
    Ok((handle, path.canonicalize().map_err(|e| e.to_string())?))
}
fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}
fn sid() -> Result<String, String> {
    let mut token = std::ptr::null_mut();
    if unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token) } == 0 {
        return Err(error());
    }
    let token = Handle(token);
    let mut size = 0;
    unsafe {
        GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &mut size);
    }
    // Aligned storage is required for TOKEN_USER, not Vec<u8>.
    let mut storage = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            storage.as_mut_ptr().cast(),
            size,
            &mut size,
        )
    } == 0
    {
        return Err(error());
    }
    let user = unsafe { &*storage.as_ptr().cast::<TOKEN_USER>() };
    let mut value = std::ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut value) } == 0 {
        return Err(error());
    }
    let _allocation = Local(value.cast());
    let mut len = 0;
    unsafe {
        while *value.add(len) != 0 {
            len += 1;
        }
    }
    Ok(String::from_utf16_lossy(unsafe {
        std::slice::from_raw_parts(value, len)
    }))
}
fn read_limited(path: &Path, max: u64) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > max {
        return Err("file too large".into());
    }
    Ok(bytes)
}

struct Managed {
    child: Child,
    owner: String,
}
struct Broker {
    dir: PathBuf,
    runtime: PathBuf,
    child: Option<Managed>,
}
impl Broker {
    fn new() -> Result<Self, String> {
        let dir = std::env::current_exe()
            .map_err(|e| e.to_string())?
            .parent()
            .ok_or("missing install dir")?
            .to_path_buf();
        no_reparse(&dir)?;
        let dir = dir.canonicalize().map_err(|e| e.to_string())?;
        // 位置无关前提（#191）：服务与托管内核二进制收紧为非管理员只读
        for name in ["crossbow-service.exe", "mihomo.exe"] {
            restrict_write_acl(&dir.join(name));
        }
        let runtime = dir.join("service-runtime");
        std::fs::create_dir_all(&runtime).map_err(|e| e.to_string())?;
        no_reparse(&runtime)?;
        let mut descriptor = std::ptr::null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)").as_ptr(),
                1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(error());
        }
        let _descriptor = Local(descriptor);
        if unsafe {
            SetFileSecurityW(
                wide(runtime.as_os_str()).as_ptr(),
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                descriptor,
            )
        } == 0
        {
            return Err(error());
        }
        Ok(Self {
            dir,
            runtime,
            child: None,
        })
    }
    fn reap(&mut self) {
        if self
            .child
            .as_mut()
            .is_some_and(|c| matches!(c.child.try_wait(), Ok(Some(_))))
        {
            self.child = None;
        }
    }
    fn stop(&mut self) {
        if let Some(mut managed) = self.child.take() {
            let _ = managed.child.kill();
            let _ = managed.child.wait();
        }
    }
    fn serve(&mut self) {
        // Interactive local users only. Remote pipe clients are rejected separately.
        let mut descriptor = std::ptr::null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)").as_ptr(),
                1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return;
        }
        let _descriptor = Local(descriptor);
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let pipe = unsafe {
            CreateNamedPipeW(
                text(PIPE).as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                8192,
                8192,
                0,
                &attributes,
            )
        };
        if pipe == INVALID_HANDLE_VALUE || pipe.is_null() {
            return;
        }
        // Retain the first-instance handle for the broker's entire lifetime.
        // Recreating it per request races the previous client's CloseHandle and
        // lets another process squat on the pipe between requests.
        let pipe = Handle(pipe);
        while !STOP.load(Ordering::SeqCst) {
            loop {
                if STOP.load(Ordering::SeqCst) {
                    break;
                }
                let connected = unsafe { ConnectNamedPipe(pipe.0, std::ptr::null_mut()) } != 0;
                let code = unsafe { GetLastError() };
                if connected || code == ERROR_PIPE_CONNECTED {
                    self.serve_one(pipe.0);
                    break;
                }
                if code != ERROR_PIPE_LISTENING && code != ERROR_NO_DATA {
                    break;
                }
                self.reap();
                std::thread::sleep(Duration::from_millis(20));
            }
            unsafe {
                DisconnectNamedPipe(pipe.0);
            }
        }
    }
    fn serve_one(&mut self, pipe: HANDLE) {
        let reply = (|| -> Result<serde_json::Value, String> {
            let line = read_line(pipe, 4096, Duration::from_secs(5))?;
            let mut pid = 0;
            if unsafe { GetNamedPipeClientProcessId(pipe, &mut pid) } == 0 { return Err(error()); }
            let (_process, path) = process_path(pid)?;
            let expected = self.dir.join("crossbow-desktop.exe").canonicalize().map_err(|e| e.to_string())?;
            if !same_path(&path, &expected) { return Err("only the installed Crossbow GUI may use this service".into()); }
            let impersonation = Impersonation::enter(pipe)?;
            let owner = sid()?;
            let request: serde_json::Value = serde_json::from_slice(&line).map_err(|_| "invalid request")?;
            self.reap();
            if self.child.as_ref().is_some_and(|c| c.owner != owner) { return Err("core belongs to another user".into()); }
            match request.get("cmd").and_then(|c| c.as_str()) {
                Some("status") => Ok(serde_json::json!({"ok":true,"running":self.child.is_some(),"pid":self.child.as_ref().map(|c|c.child.id())})),
                Some("stop") => {
                    let had = self.child.is_some(); drop(impersonation); self.stop();
                    if had { Ok(serde_json::json!({"ok":true})) } else { Err("no core".into()) }
                }
                Some("start") => {
                    if self.child.is_some() { return Err("core already running".into()); }
                    let field = |key| request.get(key).and_then(|v|v.as_str()).map(PathBuf::from).ok_or_else(|| format!("missing {key}"));
                    let binary = field("binary")?.canonicalize().map_err(|e| e.to_string())?;
                    let allowed = self.dir.join("mihomo.exe").canonicalize().map_err(|e| e.to_string())?;
                    if !same_path(&binary, &allowed) { return Err("only the installed mihomo.exe is allowed".into()); }
                    let source = field("config")?;
                    let base = source.parent().ok_or("invalid config path")?;
                    let bytes = read_limited(&source, crate::policy::MAX_CONFIG)?;
                    let (yaml, mut paths) = crate::policy::config(&bytes)?;
                    paths.extend(["GeoIP.dat", "GeoSite.dat", "geoip.metadb", "geosite.db", "country.mmdb", "Country.mmdb", "ASN.mmdb"].map(str::to_string));
                    let mut assets = Vec::new(); let mut total = 0;
                    for path in paths {
                        let input = base.join(&path);
                        match read_limited(&input, crate::policy::MAX_ASSET) {
                            Ok(bytes) => { total += bytes.len(); if total > 128 * 1024 * 1024 { return Err("provider assets too large".into()); } assets.push((path,bytes)); },
                            Err(_) if !input.exists() => {},
                            Err(e) => return Err(e),
                        }
                    }
                    // The supplied path is only opened with the caller's token, never SYSTEM.
                    let log = std::fs::OpenOptions::new().create(true).write(true).truncate(true)
                        .open(field("log")?).map_err(|e| e.to_string())?;
                    let stderr = log.try_clone().map_err(|e| e.to_string())?;
                    drop(impersonation);
                    no_reparse(&self.runtime)?;
                    // A second Windows account must not inherit another user's
                    // provider/cache data or controller configuration.
                    let runtime = self.runtime.join(&owner);
                    std::fs::create_dir_all(&runtime).map_err(|e| e.to_string())?;
                    no_reparse(&runtime)?;
                    for (path,bytes) in assets {
                        let output = runtime.join(path);
                        std::fs::create_dir_all(output.parent().ok_or("invalid asset path")?).map_err(|e|e.to_string())?;
                        no_reparse(output.parent().unwrap())?;
                        if output.exists() { no_reparse(&output)?; }
                        std::fs::write(output, bytes).map_err(|e|e.to_string())?;
                    }
                    let config = runtime.join("runtime.yaml");
                    if config.exists() { no_reparse(&config)?; }
                    std::fs::write(&config, yaml).map_err(|e|e.to_string())?;
                    let child = crate::core_process::command(&allowed, &runtime, &config)?
                        .stdout(log).stderr(stderr).spawn().map_err(|e|e.to_string())?;
                    let pid = child.id(); self.child = Some(Managed { child, owner });
                    Ok(serde_json::json!({"ok":true,"pid":pid}))
                }
                _ => Err("unknown command".into())
            }
        })().unwrap_or_else(|error|serde_json::json!({"ok":false,"error":error}));
        let out = format!("{reply}\n");
        let mut written = 0;
        if unsafe {
            WriteFile(
                pipe,
                out.as_ptr(),
                out.len() as u32,
                &mut written,
                std::ptr::null_mut(),
            )
        } != 0
            && written as usize == out.len()
        {
            // Client ACK avoids discarding a buffered reply on DisconnectNamedPipe;
            // unlike FlushFileBuffers, a silent client cannot hang SCM shutdown.
            let _ = read_line(pipe, 8, Duration::from_secs(2));
        }
    }
}
fn read_line(pipe: HANDLE, limit: usize, timeout: Duration) -> Result<Vec<u8>, String> {
    let deadline = Instant::now() + timeout;
    let mut line = Vec::new();
    loop {
        if STOP.load(Ordering::SeqCst) || Instant::now() >= deadline {
            return Err("service pipe timeout".into());
        }
        let mut byte = 0u8;
        let mut read = 0;
        let ok = unsafe { ReadFile(pipe, &mut byte, 1, &mut read, std::ptr::null_mut()) };
        if ok != 0 && read == 1 {
            if byte == b'\n' {
                return Ok(line);
            }
            if line.len() >= limit {
                return Err("request too large".into());
            }
            line.push(byte);
        } else if unsafe { GetLastError() } == ERROR_NO_DATA || (ok != 0 && read == 0) {
            std::thread::sleep(Duration::from_millis(10));
        } else {
            return Err(error());
        }
    }
}

pub fn run() -> i32 {
    let mut name = text("CrossbowService");
    let mut table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: name.as_mut_ptr(),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: std::ptr::null_mut(),
            lpServiceProc: None,
        },
    ];
    if unsafe { StartServiceCtrlDispatcherW(table.as_mut_ptr()) } == 0 {
        1
    } else {
        0
    }
}
