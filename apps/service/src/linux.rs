//! A single-user TUN session. The system manager grants only CAP_NET_ADMIN;
//! the GUI owns a live socket, while this process owns the core and its routes.
use crossbow_service::linux_network::{
    self as network, WireguardPlan, DEVICE, MARK, PRIORITY, PROTOCOL, TABLE, WIREGUARD_PRIORITY,
    WIREGUARD_TABLE,
};
use serde_json::{json, Value};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const INSTALL: &str = "/usr/lib/crossbow";
const SERVICE: &str = "/usr/lib/crossbow/crossbow-service";
const CORE: &str = "/usr/lib/crossbow/mihomo";
const RUNTIME: &str = "/run/crossbow-tun";
const SOCKET: &str = "/run/crossbow-tun/control.sock";
const JOURNAL: &str = "/run/crossbow-tun/network.json";
const SESSION: &str = "/run/crossbow-tun/session";
const FRAME_LIMIT: usize = 4096;
static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn stop_signal(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}
fn uid() -> u32 {
    unsafe { libc::geteuid() }
}

pub fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [mode] if mode == "--authorize" => authorize(),
        [mode] if mode == "--serve" => serve(),
        [mode] if mode == "--cleanup" => cleanup(),
        _ => Err("expected --authorize, --serve or --cleanup".into()),
    }
}

fn protected_install() -> Result<(), String> {
    for path in ["/usr", "/usr/lib", INSTALL, SERVICE, CORE] {
        let meta = fs::symlink_metadata(path).map_err(|e| format!("安装不完整：{path}: {e}"))?;
        if meta.file_type().is_symlink() || meta.uid() != 0 || meta.mode() & 0o022 != 0 {
            return Err(format!(
                "安装路径必须由 root 管理且不可被普通用户改写：{path}"
            ));
        }
        if [SERVICE, CORE].contains(&path) && (!meta.is_file() || meta.mode() & 0o111 == 0) {
            return Err(format!("安装程序不可执行：{path}"));
        }
    }
    Ok(())
}

fn authorize() -> Result<(), String> {
    if uid() != 0 {
        return Err("请通过系统授权启动 Crossbow TUN 服务".into());
    }
    let owner = std::env::var("PKEXEC_UID")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|owner| *owner > 0)
        .ok_or("缺少有效的 pkexec 调用用户")?;
    protected_install()?;
    if Path::new(JOURNAL).symlink_metadata().is_ok() {
        return Err("上次 TUN 清理尚未完成，请先由原用户恢复网络；没有覆盖残留记录".into());
    }
    if let Ok(meta) = fs::symlink_metadata(RUNTIME) {
        if !meta.is_dir() || meta.file_type().is_symlink() || meta.uid() != owner {
            return Err("TUN 运行目录属于其他用户或状态异常，请管理员核查；没有接管该目录".into());
        }
    }
    let result = Command::new("/usr/bin/systemd-run")
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin")
        .env("LC_ALL", "C")
        .args([
            "--unit=crossbow-tun",
            "--collect",
            "--quiet",
            "--service-type=exec",
        ])
        .arg(format!("--property=User={owner}"))
        .args([
            "--property=AmbientCapabilities=CAP_NET_ADMIN",
            "--property=CapabilityBoundingSet=CAP_NET_ADMIN",
            "--property=NoNewPrivileges=yes",
            "--property=ProtectSystem=full",
            "--property=PrivateTmp=yes",
            "--property=RuntimeDirectory=crossbow-tun",
            "--property=RuntimeDirectoryMode=0700",
            "--property=RuntimeDirectoryPreserve=yes",
            "--property=KillMode=control-group",
            "--property=TimeoutStopSec=10",
            "--property=UMask=0077",
            "--property=ExecStopPost=/usr/lib/crossbow/crossbow-service --cleanup",
            SERVICE,
            "--serve",
        ])
        .output()
        .map_err(|e| format!("启动系统服务失败：{e}"))?;
    if !result.status.success() {
        return Err(format!(
            "启动系统服务失败：{}",
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    Ok(())
}

fn own_directory(path: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_dir()
        || meta.file_type().is_symlink()
        || meta.uid() != uid()
        || meta.mode() & 0o077 != 0
    {
        return Err(format!("运行目录权限不正确：{}", path.display()));
    }
    Ok(())
}

fn peer_uid(stream: &UnixStream) -> Result<u32, String> {
    let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut len,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if len as usize != std::mem::size_of::<libc::ucred>() {
        return Err("invalid peer credentials".into());
    }
    Ok(credentials.uid)
}

fn reply(stream: &mut UnixStream, value: Value) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    stream.write_all(&bytes).map_err(|e| e.to_string())
}

/// Preserve partial frames across the short socket timeout used by the watchdog.
fn read_frame(stream: &mut UnixStream, buffer: &mut Vec<u8>) -> Result<Option<Value>, String> {
    loop {
        let mut byte = [0];
        match stream.read(&mut byte) {
            Ok(0) => return Err("desktop disconnected".into()),
            Ok(_) if byte[0] == b'\n' => {
                let value = serde_json::from_slice(buffer).map_err(|_| "invalid request JSON")?;
                buffer.clear();
                return Ok(Some(value));
            }
            Ok(_) => {
                if buffer.len() >= FRAME_LIMIT {
                    return Err("request too large".into());
                }
                buffer.push(byte[0]);
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Ok(None)
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => return Ok(None),
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("source must be a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("file too large".into());
    }
    Ok(bytes)
}

fn controlled_config(
    bytes: &[u8],
    wireguard_coexist: bool,
) -> Result<(String, Vec<String>, SocketAddr, String, u16), String> {
    if bytes.len() as u64 > crate::policy::MAX_CONFIG {
        return Err("config too large".into());
    }
    let mut document: serde_yaml::Value =
        serde_yaml::from_slice(bytes).map_err(|_| "invalid config YAML")?;
    document.apply_merge().map_err(|_| "invalid YAML merge")?;
    let normalized = serde_yaml::to_string(&document).map_err(|e| e.to_string())?;
    let (base, paths) = crate::policy::config(normalized.as_bytes())?;
    let mut value: serde_yaml::Value = serde_yaml::from_str(&base).map_err(|e| e.to_string())?;
    let root = value.as_mapping_mut().ok_or("config must be a mapping")?;
    let key = serde_yaml::Value::from;
    let controller: SocketAddr = root[&key("external-controller")]
        .as_str()
        .ok_or("controller missing")?
        .parse()
        .map_err(|_| "invalid controller")?;
    let secret = root[&key("secret")]
        .as_str()
        .ok_or("secret missing")?
        .to_owned();
    if secret.len() > 128
        || !secret
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("invalid controller secret".into());
    }
    let mixed = root
        .get(key("mixed-port"))
        .and_then(serde_yaml::Value::as_u64)
        .filter(|port| (1024..=65535).contains(port))
        .ok_or("invalid mixed port")?;
    if controller.port() < 1024 || controller.port() == mixed as u16 {
        return Err("invalid controller port".into());
    }
    for name in [
        "listeners",
        "tunnels",
        "port",
        "socks-port",
        "redir-port",
        "tproxy-port",
        "tuic-server",
        "ss-config",
        "vmess-config",
        "iptables",
        "ebpf",
        "ntp",
        "interface-name",
        "external-controller-cors",
        "external-doh-server",
        "external-controller-routing-mark",
    ] {
        root.remove(key(name));
    }
    root.insert(key("allow-lan"), serde_yaml::Value::Bool(false));
    root.insert(key("bind-address"), key("127.0.0.1"));
    root.insert(key("routing-mark"), serde_yaml::Value::from(MARK));
    root.insert(key("ipv6"), serde_yaml::Value::Bool(true));
    // Match the desktop runtime and initialize the shared cache before readiness.
    root.entry(key("profile"))
        .or_insert_with(|| serde_yaml::Value::Mapping(Default::default()))
        .as_mapping_mut()
        .ok_or("profile must be a mapping")?
        .insert(key("store-selected"), serde_yaml::Value::Bool(true));
    // mihomo 1.19.32 / sing-tun 0.4.27 installs IPv6 routes and an oif rule even
    // with auto-route=false if Inet6Address is nonempty. gvisor still forwards
    // IPv6 with an empty list; this service owns both families' routes instead.
    let mut tun = json!({"enable":true,"device":DEVICE,"stack":"gvisor","auto-route":false,
        "auto-redirect":false,"auto-detect-interface":true,"dns-mode":"disabled",
        "dns-hijack":["any:53","tcp://any:53"],"inet6-address":[]});
    if wireguard_coexist {
        tun["dns-hijack"] = json!([]);
    }
    root.insert(
        key("tun"),
        serde_yaml::to_value(tun).map_err(|e| e.to_string())?,
    );
    // Keep upstream DNS choices, but never let the service bind another DNS listener.
    let dns = root
        .entry(key("dns"))
        .or_insert_with(|| serde_yaml::Value::Mapping(Default::default()));
    if !dns.is_mapping() {
        return Err("dns must be a mapping".into());
    }
    let dns = dns.as_mapping_mut().unwrap();
    dns.remove(key("listen"));
    dns.insert(key("enable"), serde_yaml::Value::Bool(true));
    dns.insert(
        key("enhanced-mode"),
        key(if wireguard_coexist {
            "redir-host"
        } else {
            "fake-ip"
        }),
    );
    dns.entry(key("nameserver"))
        .or_insert_with(|| serde_yaml::to_value(["https://1.1.1.1/dns-query"]).unwrap());
    let text = serde_yaml::to_string(&value).map_err(|e| e.to_string())?;
    Ok((text, paths, controller, secret, mixed as u16))
}

fn ensure_ports_available(addresses: &[SocketAddr]) -> Result<(), String> {
    let mut listeners = Vec::with_capacity(addresses.len());
    for address in addresses {
        listeners.push(
            TcpListener::bind(address)
                .map_err(|e| format!("内核监听端口 {address} 已被占用或不可用：{e}"))?,
        );
    }
    Ok(())
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.uid() != uid() || meta.mode() & 0o077 != 0 {
        return Err("runtime file has unsafe permissions".into());
    }
    file.set_len(0)
        .and_then(|()| file.write_all(bytes))
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())
}

fn prepare(request: &Value, wireguard_coexist: bool) -> Result<(SocketAddr, String, File), String> {
    let config = request["config"]
        .as_str()
        .map(Path::new)
        .filter(|p| p.is_absolute())
        .ok_or("config must be an absolute path")?;
    let log = request["log"]
        .as_str()
        .map(Path::new)
        .filter(|p| p.is_absolute())
        .ok_or("log must be an absolute path")?;
    let bytes = read_limited(config, crate::policy::MAX_CONFIG)?;
    let (yaml, paths, controller, secret, mixed) = controlled_config(&bytes, wireguard_coexist)?;
    fs::create_dir(SESSION).map_err(|e| format!("建立 TUN 工作目录失败：{e}"))?;
    fs::set_permissions(SESSION, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    own_directory(Path::new(SESSION))?;
    let base = config.parent().ok_or("missing configuration directory")?;
    copy_assets(base, Path::new(SESSION), paths)?;
    link_cache(base, Path::new(SESSION))?;
    write_private(&Path::new(SESSION).join("runtime.yaml"), yaml.as_bytes())?;
    // This process has the requesting user's filesystem privileges, never root's.
    let log = OpenOptions::new()
        .create(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(log)
        .map_err(|e| e.to_string())?;
    let meta = log.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.uid() != uid() {
        return Err("log must be a user-owned regular file".into());
    }
    log.set_len(0).map_err(|e| e.to_string())?;
    ensure_ports_available(&[SocketAddr::from((Ipv4Addr::LOCALHOST, mixed)), controller])?;
    Ok((controller, secret, log))
}

fn copy_assets(base: &Path, session: &Path, mut paths: Vec<String>) -> Result<(), String> {
    paths.extend(
        [
            // The desktop downloads these lowercase names (core_manager::GEO_FILES).
            "geoip.metadb",
            "geosite.dat",
            "GeoIP.dat",
            "GeoSite.dat",
            "geosite.db",
            "country.mmdb",
            "Country.mmdb",
            "ASN.mmdb",
        ]
        .map(str::to_string),
    );
    paths.sort();
    paths.dedup();
    let mut total = 0usize;
    for relative in paths {
        let source = base.join(&relative);
        let bytes = match read_limited(&source, crate::policy::MAX_ASSET) {
            Ok(bytes) => bytes,
            Err(_) if !source.exists() => continue,
            Err(e) => return Err(e),
        };
        total += bytes.len();
        if total > 128 * 1024 * 1024 {
            return Err("provider assets too large".into());
        }
        let output = session.join(relative);
        fs::create_dir_all(output.parent().unwrap()).map_err(|e| e.to_string())?;
        write_private(&output, &bytes)?;
    }
    Ok(())
}

fn link_cache(base: &Path, session: &Path) -> Result<(), String> {
    let target = fs::canonicalize(base)
        .map_err(|e| e.to_string())?
        .join("cache.db");
    let cache = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&target)
        .map_err(|e| format!("无法打开节点选择缓存：{e}"))?;
    let meta = cache.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.uid() != uid() {
        return Err("节点选择缓存必须是当前用户的普通文件".into());
    }
    if unsafe { libc::flock(cache.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("节点选择缓存仍被占用，请等待原内核退出后重试".into());
    }
    // Both modes use the same database. Removing SESSION only removes this link,
    // so selection writes survive service/core crashes without a copy-back step.
    std::os::unix::fs::symlink(target, session.join("cache.db"))
        .map_err(|e| format!("无法共享节点选择缓存：{e}"))
}

fn verify_cache_link(session: &Path) -> Result<(), String> {
    if !fs::symlink_metadata(session.join("cache.db"))
        .map_err(|e| e.to_string())?
        .file_type()
        .is_symlink()
    {
        // The pinned core replaces a corrupt database by unlinking its path.
        return Err("mihomo 未使用共享节点缓存；请先在显式代理模式修复缓存后重试".into());
    }
    Ok(())
}

fn cache_ready(session: &Path) -> Result<bool, String> {
    verify_cache_link(session)?;
    let cache = File::open(session.join("cache.db")).map_err(|e| e.to_string())?;
    if unsafe { libc::flock(cache.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        // Dropping this descriptor releases the probe lock. /version can be
        // available before providers and the selected-node cache have loaded.
        return Ok(false);
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::WouldBlock {
        Ok(true)
    } else {
        Err(format!("无法确认节点选择缓存就绪：{error}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Resource {
    Route(u8, usize),
    Rule(u8),
    WireguardRule(u8),
    WireguardCapture(u8),
    WireguardRoute {
        prefix: String,
        link: Option<(String, u32)>,
    },
    WireguardEndpoint(IpAddr),
}

const WIREGUARD_METRIC: u32 = 1024;
const WIREGUARD_UNREACHABLE_METRIC: u32 = 4096;
const MAX_RESOURCES: usize = 256;
const V4: &[&str] = &[
    "default",
    "10.0.0.0/8",
    "100.64.0.0/10",
    "127.0.0.0/8",
    "169.254.0.0/16",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "224.0.0.0/4",
    "255.255.255.255/32",
];
const V6: &[&str] = &["default", "::1/128", "fc00::/7", "fe80::/10", "ff00::/8"];
impl Resource {
    fn id(&self) -> String {
        match self {
            Self::Route(f, i) => format!("{f}:route:{i}"),
            Self::Rule(f) => format!("{f}:rule"),
            Self::WireguardRule(f) => format!("{f}:wg-rule"),
            Self::WireguardCapture(f) => format!("{f}:wg-capture"),
            Self::WireguardRoute { prefix, link: None } => format!("wg-block|{prefix}"),
            Self::WireguardRoute {
                prefix,
                link: Some((name, index)),
            } => {
                format!("wg-route|{name}|{index}|{prefix}")
            }
            Self::WireguardEndpoint(endpoint) => format!("wg-endpoint|{endpoint}"),
        }
    }
    fn all() -> Vec<Self> {
        [4, 6]
            .into_iter()
            .flat_map(|f| (0..prefixes(f).len()).map(move |i| Self::Route(f, i)))
            .chain([Self::Rule(4), Self::Rule(6)])
            .collect()
    }
    fn for_session(wireguard: Option<&WireguardPlan>) -> Vec<Self> {
        let mut resources = Vec::new();
        if let Some(plan) = wireguard {
            // Both routes are Crossbow-owned. If the kernel removes the device
            // route, the less preferred unreachable route still blocks fallback.
            for prefix in &plan.prefixes {
                resources.push(Self::WireguardRoute {
                    prefix: prefix.clone(),
                    link: None,
                });
                resources.push(Self::WireguardRoute {
                    prefix: prefix.clone(),
                    link: Some((plan.device.clone(), plan.ifindex)),
                });
            }
            resources.extend([Self::WireguardRule(4), Self::WireguardRule(6)]);
            resources.extend(plan.endpoints.iter().copied().map(Self::WireguardEndpoint));
        }
        // Install public capture last. Recovery removes it first.
        resources.extend(Self::all().into_iter().map(|resource| match resource {
            Self::Rule(family) if wireguard.is_some() => Self::WireguardCapture(family),
            resource => resource,
        }));
        resources
    }
    fn parse(id: &str, version: u64) -> Option<Self> {
        if let Some(resource) = Self::all().into_iter().find(|r| r.id() == id) {
            return Some(resource);
        }
        if version != 2 {
            return None;
        }
        let parts: Vec<_> = id.split('|').collect();
        let resource = match parts.as_slice() {
            ["4:wg-rule"] => Self::WireguardRule(4),
            ["6:wg-rule"] => Self::WireguardRule(6),
            ["4:wg-capture"] => Self::WireguardCapture(4),
            ["6:wg-capture"] => Self::WireguardCapture(6),
            ["wg-block", prefix] => Self::WireguardRoute {
                prefix: network::normalize_wireguard_prefix(prefix)?,
                link: None,
            },
            ["wg-route", name, index, prefix] if network::valid_wireguard_device(name) => {
                let index = index.parse::<u32>().ok().filter(|index| *index > 0)?;
                Self::WireguardRoute {
                    prefix: network::normalize_wireguard_prefix(prefix)?,
                    link: Some((name.to_string(), index)),
                }
            }
            ["wg-endpoint", endpoint] => {
                let endpoint = endpoint.parse::<IpAddr>().ok()?;
                let prefix = Self::endpoint_prefix(endpoint);
                network::normalize_wireguard_prefix(&prefix)?;
                Self::WireguardEndpoint(endpoint)
            }
            _ => return None,
        };
        (resource.id() == id).then_some(resource)
    }
    fn endpoint_prefix(endpoint: IpAddr) -> String {
        let length = if endpoint.is_ipv4() { 32 } else { 128 };
        format!("{endpoint}/{length}")
    }
    fn family(&self) -> u8 {
        match self {
            Self::Route(f, _)
            | Self::Rule(f)
            | Self::WireguardRule(f)
            | Self::WireguardCapture(f) => *f,
            Self::WireguardRoute { prefix, .. } if prefix.contains(':') => 6,
            Self::WireguardEndpoint(IpAddr::V6(_)) => 6,
            _ => 4,
        }
    }
    fn is_rule(&self) -> bool {
        matches!(
            self,
            Self::Rule(_) | Self::WireguardRule(_) | Self::WireguardCapture(_)
        )
    }
    fn table(&self) -> u32 {
        match self {
            Self::WireguardRule(_) | Self::WireguardRoute { .. } => WIREGUARD_TABLE,
            _ => TABLE,
        }
    }
    fn args(&self, action: &str) -> Vec<String> {
        let mut args = vec![format!("-{}", self.family())];
        match self {
            Self::WireguardRule(_) | Self::WireguardCapture(_) => {
                args.extend([
                    "rule".into(),
                    action.into(),
                    "from".into(),
                    "all".into(),
                    "iif".into(),
                    "lo".into(),
                ]);
                let priority = if matches!(self, Self::WireguardCapture(_)) {
                    args.extend(["fwmark".into(), "0/0xffffffff".into()]);
                    PRIORITY
                } else {
                    WIREGUARD_PRIORITY
                };
                args.extend([
                    "table".into(),
                    self.table().to_string(),
                    "priority".into(),
                    priority.to_string(),
                    "protocol".into(),
                    PROTOCOL.to_string(),
                ]);
            }
            Self::WireguardRoute { prefix, link } => {
                args.extend(["route".into(), action.into()]);
                if link.is_none() {
                    args.push("unreachable".into());
                }
                args.push(prefix.clone());
                if let Some((device, _)) = link {
                    args.extend(["dev".into(), device.clone()]);
                }
                let metric = if link.is_some() {
                    WIREGUARD_METRIC
                } else {
                    WIREGUARD_UNREACHABLE_METRIC
                };
                args.extend([
                    "table".into(),
                    WIREGUARD_TABLE.to_string(),
                    "proto".into(),
                    PROTOCOL.to_string(),
                    "metric".into(),
                    metric.to_string(),
                ]);
            }
            Self::WireguardEndpoint(endpoint) => {
                args.extend([
                    "route".into(),
                    action.into(),
                    "throw".into(),
                    Self::endpoint_prefix(*endpoint),
                    "table".into(),
                    TABLE.to_string(),
                    "proto".into(),
                    PROTOCOL.to_string(),
                ]);
                if endpoint.is_ipv6() {
                    args.extend(["metric".into(), "1024".into()]);
                }
            }
            Self::Rule(_) => args.extend([
                "rule".into(),
                action.into(),
                "not".into(),
                "from".into(),
                "all".into(),
                "fwmark".into(),
                MARK.to_string(),
                "table".into(),
                TABLE.to_string(),
                "priority".into(),
                PRIORITY.to_string(),
                "protocol".into(),
                PROTOCOL.to_string(),
            ]),
            Self::Route(f, index) => {
                args.extend(["route".into(), action.into()]);
                if *index != 0 {
                    args.push("throw".into());
                }
                args.push(prefixes(*f)[*index].into());
                if *index == 0 {
                    args.extend(["dev".into(), DEVICE.into()]);
                }
                args.extend([
                    "table".into(),
                    TABLE.to_string(),
                    "proto".into(),
                    PROTOCOL.to_string(),
                ]);
                if *f == 6 {
                    args.extend(["metric".into(), "1024".into()]);
                }
            }
        }
        args
    }
    fn matches(&self, row: &Value) -> bool {
        let number = |key: &str, n: u32| {
            row[key].as_u64() == Some(n as u64)
                || row[key].as_str().and_then(|s| s.parse::<u32>().ok()) == Some(n)
        };
        if !number("protocol", PROTOCOL) {
            return false;
        }
        match self {
            Self::WireguardRule(_) | Self::WireguardCapture(_) => {
                let capture = matches!(self, Self::WireguardCapture(_));
                number(
                    "priority",
                    if capture {
                        PRIORITY
                    } else {
                        WIREGUARD_PRIORITY
                    },
                ) && number("table", self.table())
                    && row["src"] == "all"
                    && row["iif"] == "lo"
                    && (!capture
                        || (number("fwmark", 0)
                            && row.get("fwmask").is_none_or(|mask| mask == "0xffffffff")))
                    && row.as_object().is_some_and(|fields| {
                        fields.keys().all(|key| {
                            matches!(
                                key.as_str(),
                                "priority" | "src" | "table" | "protocol" | "iif"
                            ) || (capture && matches!(key.as_str(), "fwmark" | "fwmask"))
                        })
                    })
            }
            Self::WireguardRoute { prefix, link } => {
                let (kind, device, metric) = if let Some((device, _)) = link {
                    ("unicast", Some(device.as_str()), WIREGUARD_METRIC)
                } else {
                    (
                        "unreachable",
                        (self.family() == 6).then_some("lo"),
                        WIREGUARD_UNREACHABLE_METRIC,
                    )
                };
                self.matches_route(row, prefix, kind, device, metric)
            }
            Self::WireguardEndpoint(endpoint) => self.matches_route(
                row,
                &Self::endpoint_prefix(*endpoint),
                "throw",
                endpoint.is_ipv6().then_some("lo"),
                if endpoint.is_ipv6() { 1024 } else { 0 },
            ),
            Self::Rule(_) => {
                number("priority", PRIORITY)
                    && number("table", TABLE)
                    && row.get("not").is_some()
                    && row["src"] == "all"
                    && row["fwmark"] == format!("0x{MARK:x}")
                    && row.get("fwmask").is_none_or(|mask| mask == "0xffffffff")
                    && row.as_object().is_some_and(|m| {
                        m.keys().all(|key| {
                            matches!(
                                key.as_str(),
                                "priority"
                                    | "table"
                                    | "protocol"
                                    | "not"
                                    | "fwmark"
                                    | "fwmask"
                                    | "src"
                            )
                        })
                    })
            }
            Self::Route(f, index) => {
                let prefix = prefixes(*f)[*index];
                // iproute2 omits the host-length suffix in its JSON output.
                let printed = prefix
                    .strip_suffix(if *f == 4 { "/32" } else { "/128" })
                    .unwrap_or(prefix);
                row["dst"] == printed
                    && row.as_object().is_some_and(|m| {
                        m.keys().all(|key| {
                            matches!(
                                key.as_str(),
                                "dst"
                                    | "dev"
                                    | "protocol"
                                    | "scope"
                                    | "flags"
                                    | "metric"
                                    | "pref"
                                    | "type"
                                    | "table"
                            )
                        })
                    })
                    && if *f == 6 {
                        number("metric", 1024)
                    } else {
                        row.get("metric").is_none_or(|metric| metric == 0)
                    }
                    && if *index == 0 {
                        row["dev"] == DEVICE && row.get("type").is_none_or(|kind| kind == "unicast")
                    } else {
                        row["type"] == "throw"
                            && if *f == 6 {
                                row["dev"] == "lo"
                            } else {
                                row.get("dev").is_none()
                            }
                    }
            }
        }
    }
    fn matches_route(
        &self,
        row: &Value,
        prefix: &str,
        kind: &str,
        device: Option<&str>,
        metric: u32,
    ) -> bool {
        let printed = prefix.strip_suffix(if self.family() == 4 { "/32" } else { "/128" });
        (row["dst"] == prefix || printed.is_some_and(|ip| row["dst"] == ip))
            && row["type"].as_str().unwrap_or("unicast") == kind
            && row["dev"].as_str() == device
            && row["metric"].as_u64().unwrap_or(0) == u64::from(metric)
            && row.as_object().is_some_and(|fields| {
                fields.keys().all(|key| {
                    matches!(
                        key.as_str(),
                        "dst"
                            | "dev"
                            | "protocol"
                            | "scope"
                            | "flags"
                            | "metric"
                            | "pref"
                            | "type"
                            | "table"
                    )
                })
            })
    }
    fn exists(&self) -> Result<bool, String> {
        self.exists_in(&network::routing_rows(self.family(), self.is_rule())?)
    }
    fn exists_in(&self, rows: &[Value]) -> Result<bool, String> {
        let count = rows
            .iter()
            .filter(|row| in_table(row, self.table()) && self.matches(row))
            .count();
        if count > 1 {
            return Err("发现重复网络资源，保留记录供核查".into());
        }
        Ok(count == 1)
    }
}
fn in_table(row: &Value, table: u32) -> bool {
    row["table"].as_u64() == Some(u64::from(table))
        || row["table"]
            .as_str()
            .and_then(|value| value.parse::<u32>().ok())
            == Some(table)
}
fn prefixes(family: u8) -> &'static [&'static str] {
    if family == 4 {
        V4
    } else {
        V6
    }
}
fn change(resource: &Resource, action: &str) -> Result<(), String> {
    let args = resource.args(action);
    network::ip(&args.iter().map(String::as_str).collect::<Vec<_>>())
        .map(|_| ())
        .map_err(|error| {
            if matches!(resource, Resource::Route(_, _) | Resource::Rule(_)) {
                error
            } else if action == "add" {
                "无法添加 WireGuard 共存路由资源".into()
            } else {
                "无法清理 WireGuard 共存路由资源".into()
            }
        })
}

struct Journal {
    ifindex: u32,
    resources: Vec<Resource>,
    pending: Option<Resource>,
}
impl Journal {
    fn decode(bytes: &[u8]) -> Result<Self, String> {
        let value: Value = serde_json::from_slice(bytes).map_err(|_| "网络恢复记录损坏")?;
        let version = value["version"]
            .as_u64()
            .filter(|version| matches!(version, 1 | 2))
            .ok_or("网络恢复记录版本不受支持")?;
        if value["uid"] != uid() {
            return Err("网络恢复记录不属于当前服务用户".into());
        }
        let ifindex = value["ifindex"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n > 0)
            .ok_or("invalid interface record")?;
        let mut resources = Vec::new();
        let ids = value["resources"].as_array().ok_or("invalid resources")?;
        if ids.len() > MAX_RESOURCES {
            return Err("too many network resources".into());
        }
        for id in ids {
            let resource = id
                .as_str()
                .and_then(|id| Resource::parse(id, version))
                .ok_or("unknown resource in journal")?;
            if resources.contains(&resource) {
                return Err("duplicate journal resource".into());
            }
            resources.push(resource);
        }
        let pending = if value["pending"].is_null() {
            None
        } else {
            Some(
                value["pending"]
                    .as_str()
                    .and_then(|id| Resource::parse(id, version))
                    .ok_or("invalid pending resource")?,
            )
        };
        Ok(Self {
            ifindex,
            resources,
            pending,
        })
    }
    fn save(&self) -> Result<(), String> {
        let has_wireguard = self
            .resources
            .iter()
            .chain(self.pending.as_ref())
            .any(|resource| !matches!(resource, Resource::Route(_, _) | Resource::Rule(_)));
        let value = json!({"version":if has_wireguard { 2 } else { 1 },"uid":uid(),"ifindex":self.ifindex,
            "resources":self.resources.iter().map(Resource::id).collect::<Vec<_>>(),
            "pending":self.pending.as_ref().map(Resource::id)});
        let temp = Path::new(RUNTIME).join("network.json.tmp");
        write_private(
            &temp,
            &serde_json::to_vec(&value).map_err(|e| e.to_string())?,
        )?;
        fs::rename(&temp, JOURNAL).map_err(|e| e.to_string())?;
        File::open(RUNTIME)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())
    }
    fn install(&mut self, resources: &[Resource]) -> Result<(), String> {
        self.save()?;
        for resource in resources {
            self.install_one(
                resource.clone(),
                || resource.exists(),
                || change(resource, "add"),
                Self::save,
            )?;
        }
        Ok(())
    }
    fn install_one(
        &mut self,
        resource: Resource,
        exists: impl FnOnce() -> Result<bool, String>,
        add: impl FnOnce() -> Result<(), String>,
        mut save: impl FnMut(&Self) -> Result<(), String>,
    ) -> Result<(), String> {
        if exists()? {
            return Err("所需网络资源已存在，没有接管该资源".into());
        }
        self.pending = Some(resource.clone());
        save(self)?;
        if let Err(error) = add() {
            // An unsuccessful add (notably EEXIST) does not establish ownership.
            self.pending = None;
            save(self).map_err(|persist| format!("{error}；恢复记录保存失败：{persist}"))?;
            return Err(error);
        }
        self.resources.push(resource);
        self.pending = None;
        save(self)
    }
}

fn cleanup() -> Result<(), String> {
    if !Path::new(RUNTIME).exists() {
        return Ok(());
    }
    own_directory(Path::new(RUNTIME))?;
    if Path::new(JOURNAL).symlink_metadata().is_ok() {
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(JOURNAL)
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(65537)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 65536 {
            return Err("network journal too large".into());
        }
        let mut journal = Journal::decode(&bytes)?;
        if network::device_index()?.is_some_and(|index| index != journal.ifindex) {
            return Err("Crossbow 接口身份已变化，没有删除其他接口或网络记录".into());
        }
        while let Some(resource) = journal.resources.last() {
            if resource.exists()? {
                if let Resource::WireguardRoute {
                    link: Some((name, index)),
                    ..
                } = resource
                {
                    if network::interface_index(name)? != Some(*index) {
                        return Err("WireGuard 接口身份已变化，保留待核查的网络记录".into());
                    }
                }
                change(resource, "del")?;
                if resource.exists()? {
                    return Err("网络资源尚未移除，保留恢复记录".into());
                }
            }
            journal.resources.pop();
            journal.save()?;
        }
        // An intent recorded before `ip add` is not proof that the add succeeded.
        // In the crash window, prefer an explicit recovery error to deleting a
        // resource another network manager might have created concurrently.
        if let Some(pending) = &journal.pending {
            if pending.exists()? {
                return Err(
                    "网络恢复记录含有未确认创建的资源，已保留；请管理员核查并恢复后重试".into(),
                );
            }
            journal.pending = None;
            journal.save()?;
        }
        if network::device_index()? == Some(journal.ifindex) {
            network::ip(&["link", "delete", "dev", DEVICE])?;
        }
        fs::remove_file(JOURNAL).map_err(|e| e.to_string())?;
    }
    if Path::new(SESSION).symlink_metadata().is_ok() {
        own_directory(Path::new(SESSION))?;
        fs::remove_dir_all(SESSION).map_err(|e| e.to_string())?;
    }
    Ok(())
}

struct Session {
    child: Option<Child>,
    ifindex: Option<u32>,
    wireguard: Option<WireguardPlan>,
    resources: Vec<Resource>,
}
impl Session {
    fn new() -> Self {
        Self {
            child: None,
            ifindex: None,
            wireguard: None,
            resources: Vec::new(),
        }
    }
    fn start(&mut self, request: &Value) -> Result<(), String> {
        let enabled = match request.get("wireguard_coexist") {
            None => false,
            Some(value) => value
                .as_bool()
                .ok_or("invalid WireGuard coexistence setting")?,
        };
        let own_pids = [std::process::id()];
        self.wireguard = if enabled {
            Some(network::wireguard_plan(None, &own_pids)?)
        } else {
            network::ensure_available(None, &own_pids)?;
            None
        };
        self.resources = Resource::for_session(self.wireguard.as_ref());
        if self.resources.len() > MAX_RESOURCES {
            return Err("WireGuard 共存所需的路由资源过多".into());
        }
        let (controller, secret, log) = prepare(request, enabled)?;
        let stderr = log.try_clone().map_err(|e| e.to_string())?;
        let child = Command::new(CORE)
            .args([
                "-d",
                SESSION,
                "-f",
                "/run/crossbow-tun/session/runtime.yaml",
            ])
            .current_dir(SESSION)
            .env_clear()
            // Older pinned cores ignore dns-mode and discover resolvectl through
            // PATH. Only the two installed binaries are in this root-owned dir.
            .env("PATH", INSTALL)
            .env("HOME", SESSION)
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(stderr)
            .spawn()
            .map_err(|e| format!("启动 mihomo 失败：{e}"))?;
        let child = self.child.insert(child);
        let deadline = Instant::now() + Duration::from_secs(20);
        let ifindex = loop {
            if STOP.load(Ordering::Relaxed) {
                return Err("服务正在停止".into());
            }
            if child.try_wait().map_err(|e| e.to_string())?.is_some() {
                return Err("mihomo 在就绪前退出，请查看内核日志".into());
            }
            if let Some(index) = network::device_index()? {
                if self.ifindex != Some(index) {
                    self.ifindex = Some(index);
                    // Record interface identity before waiting for the controller.
                    Journal {
                        ifindex: index,
                        resources: Vec::new(),
                        pending: None,
                    }
                    .save()?;
                }
                if controller_ready(controller, &secret) && cache_ready(Path::new(SESSION))? {
                    break index;
                }
            }
            if Instant::now() >= deadline {
                return Err("等待 TUN 设备、内核控制器和节点缓存超时".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        self.verify_network()?;
        Journal {
            ifindex,
            resources: Vec::new(),
            pending: None,
        }
        .install(&self.resources)?;
        self.verify_network()?;
        self.verify_session()
    }
    fn stop(&mut self) -> Result<(), String> {
        if let Some(child) = self.child.as_mut() {
            if child.try_wait().map_err(|e| e.to_string())?.is_none() {
                unsafe {
                    libc::kill(child.id() as i32, libc::SIGTERM);
                }
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    if child.try_wait().map_err(|e| e.to_string())?.is_some() {
                        break;
                    }
                    if Instant::now() >= deadline {
                        child.kill().map_err(|e| format!("停止内核失败：{e}"))?;
                        let deadline = Instant::now() + Duration::from_secs(2);
                        loop {
                            if child.try_wait().map_err(|e| e.to_string())?.is_some() {
                                break;
                            }
                            if Instant::now() >= deadline {
                                return Err("内核未确认退出，保留会话和恢复记录".into());
                            }
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            self.child = None;
        }
        cleanup()?;
        self.ifindex = None;
        self.wireguard = None;
        self.resources.clear();
        Ok(())
    }
    fn verify_network(&self) -> Result<(), String> {
        let child = self.child.as_ref().ok_or("TUN 会话缺少内核进程句柄")?;
        let ifindex = self.ifindex.ok_or("TUN 尚未就绪")?;
        let pids = [std::process::id(), child.id()];
        if let Some(expected) = &self.wireguard {
            if network::wireguard_plan(Some(ifindex), &pids)? != *expected {
                return Err("WireGuard 接口、路由或固定端点发生变化，正在退出本次接管".into());
            }
            Ok(())
        } else {
            network::ensure_available(Some(ifindex), &pids)
        }
    }
    fn status(&self) -> Value {
        json!({"ok":true,"wireguard_coexist_supported":true,"active":self.child.is_some(),"pid":self.child.as_ref().map(Child::id),"ifindex":self.ifindex,"stopped":self.child.is_none() && self.ifindex.is_none()})
    }
    fn verify_session(&self) -> Result<(), String> {
        if !cache_ready(Path::new(SESSION))? {
            return Err("内核节点缓存未保持打开，正在退出本次接管".into());
        }
        let expected = self.ifindex.ok_or("TUN 尚未就绪")?;
        if network::device_index()? != Some(expected) {
            return Err("TUN 接口已消失或身份发生变化".into());
        }
        for family in [4, 6] {
            let routes = network::routing_rows(family, false)?;
            let rules = network::routing_rows(family, true)?;
            let resources: Vec<_> = self
                .resources
                .iter()
                .filter(|r| r.family() == family)
                .collect();
            for resource in &resources {
                let rows = if resource.is_rule() { &rules } else { &routes };
                if !resource.exists_in(rows)? {
                    return Err("TUN 路由已变化，正在退出本次接管".into());
                }
            }
            if self.wireguard.is_some() {
                for (rows, is_rule) in [(&routes, false), (&rules, true)] {
                    if rows.iter().any(|row| {
                        (in_table(row, TABLE) || in_table(row, WIREGUARD_TABLE))
                            && !resources.iter().any(|resource| {
                                resource.is_rule() == is_rule
                                    && in_table(row, resource.table())
                                    && resource.matches(row)
                            })
                    }) {
                        return Err("Crossbow 路由表出现未登记的资源，正在退出本次接管".into());
                    }
                }
            }
        }
        Ok(())
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            eprintln!("TUN cleanup incomplete: {error}");
        }
    }
}

fn controller_ready(address: SocketAddr, secret: &str) -> bool {
    let Ok(mut socket) = TcpStream::connect_timeout(&address, Duration::from_millis(200)) else {
        return false;
    };
    let _ = socket.set_read_timeout(Some(Duration::from_millis(200)));
    let _ = socket.set_write_timeout(Some(Duration::from_millis(200)));
    if write!(socket, "GET /version HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {secret}\r\nConnection: close\r\n\r\n").is_err() { return false; }
    let mut response = [0; 128];
    socket
        .read(&mut response)
        .ok()
        .is_some_and(|n| response[..n].starts_with(b"HTTP/1.1 200"))
}

/// true means a normal stop: retain the authorized unit briefly for a restart.
fn connection(mut stream: UnixStream) -> Result<bool, String> {
    if peer_uid(&stream)? != uid() {
        return Err("service belongs to another user".into());
    }
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(|e| e.to_string())?;
    let mut buffer = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    let request = loop {
        if STOP.load(Ordering::Relaxed) || Instant::now() >= deadline {
            return Err("first request timed out".into());
        }
        if let Some(request) = read_frame(&mut stream, &mut buffer)? {
            break request;
        }
    };
    let mut session = Session::new();
    match request["cmd"].as_str() {
        Some("status") => {
            cleanup()?;
            reply(&mut stream, session.status())?;
            return Ok(true);
        }
        Some("stop") => {
            session.stop()?;
            reply(&mut stream, json!({"ok":true,"stopped":true}))?;
            return Ok(true);
        }
        Some("start") => {}
        _ => {
            reply(
                &mut stream,
                json!({"ok":false,"error":"unknown command","stopped":true}),
            )?;
            return Ok(true);
        }
    }
    if let Err(error) = session.start(&request) {
        let recovery = session.stop();
        let stopped = recovery.is_ok();
        let error = match recovery {
            Ok(()) => error,
            Err(recovery) => format!("{error}；清理尚未完成：{recovery}"),
        };
        reply(
            &mut stream,
            json!({"ok":false,"error":error,"stopped":stopped}),
        )?;
        if stopped {
            return Ok(true);
        }
    } else {
        reply(&mut stream, session.status())?;
    }
    loop {
        if STOP.load(Ordering::Relaxed) {
            eprintln!("TUN session stopping on service shutdown");
            session.stop()?;
            return Ok(false);
        }
        if let Some(child) = session.child.as_mut() {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                eprintln!("TUN session stopping because mihomo exited: {status}");
                session.stop()?;
                let _ = reply(
                    &mut stream,
                    json!({"ok":false,"error":"mihomo exited","stopped":true}),
                );
                return Ok(false);
            }
            if let Err(error) = session.verify_network() {
                eprintln!("TUN network verification failed: {error}");
                let recovery = session.stop();
                let _ = reply(
                    &mut stream,
                    json!({"ok":false,"error":error,"stopped":recovery.is_ok()}),
                );
                recovery?;
                return Ok(false);
            }
            if let Err(error) = session.verify_session() {
                eprintln!("TUN session verification failed: {error}");
                let recovery = session.stop();
                let _ = reply(
                    &mut stream,
                    json!({"ok":false,"error":error,"stopped":recovery.is_ok()}),
                );
                recovery?;
                return Ok(false);
            }
        }
        match read_frame(&mut stream, &mut buffer) {
            Ok(Some(request)) => match request["cmd"].as_str() {
                Some("status") => reply(&mut stream, session.status())?,
                Some("stop") => match session.stop() {
                    Ok(()) => {
                        reply(&mut stream, json!({"ok":true,"stopped":true}))?;
                        return Ok(true);
                    }
                    Err(error) => reply(
                        &mut stream,
                        json!({"ok":false,"error":error,"stopped":false}),
                    )?,
                },
                _ => reply(&mut stream, json!({"ok":false,"error":"unknown command"}))?,
            },
            Ok(None) => {}
            Err(error) => {
                eprintln!("TUN session stopping on control connection error: {error}");
                session.stop()?;
                return Ok(false);
            }
        }
    }
}

fn serve() -> Result<(), String> {
    own_directory(Path::new(RUNTIME))?;
    protected_install()?;
    cleanup()?;
    unsafe {
        libc::signal(
            libc::SIGTERM,
            stop_signal as *const () as libc::sighandler_t,
        );
        libc::signal(libc::SIGINT, stop_signal as *const () as libc::sighandler_t);
        libc::umask(0o077);
    }
    // A fixed systemd singleton owns this socket; never unlink another live server.
    if Path::new(SOCKET).symlink_metadata().is_ok() {
        if UnixStream::connect(SOCKET).is_ok() {
            return Err("TUN service already running".into());
        }
        let meta = fs::symlink_metadata(SOCKET).map_err(|e| e.to_string())?;
        if meta.uid() != uid() || meta.file_type().is_symlink() {
            return Err("invalid service socket".into());
        }
        fs::remove_file(SOCKET).map_err(|e| e.to_string())?;
    }
    let listener = UnixListener::bind(SOCKET).map_err(|e| e.to_string())?;
    fs::set_permissions(SOCKET, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let mut idle = Instant::now();
    let result = loop {
        if STOP.load(Ordering::Relaxed) || idle.elapsed() >= Duration::from_secs(30) {
            break Ok(());
        }
        match listener.accept() {
            Ok((stream, _)) => match connection(stream) {
                Ok(true) => idle = Instant::now(),
                Ok(false) => break Ok(()),
                Err(error) => break Err(error),
            },
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100))
            }
            Err(error) => break Err(error.to_string()),
        }
    };
    let _ = fs::remove_file(SOCKET);
    let recovered = cleanup();
    result.and(recovered)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn test_dir() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "crossbow-service-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        path
    }
    fn base() -> &'static str {
        "mixed-port: 19080\nexternal-controller: 127.0.0.1:19090\nsecret: abcdef0123456789abcdef\nrules: [MATCH,DIRECT]\n"
    }
    #[test]
    fn privileged_config_replaces_network_controls_after_yaml_merge() {
        let input = format!("{}defaults: &d {{auto-route: true, auto-redirect: true, device: foreign}}\ntun: {{<<: *d}}\nlisteners: [{{type: tun, name: extra}}]\niptables: {{enable: true}}\nrouting-mark: 1\nallow-lan: true\ndns: {{enable: false, listen: '0.0.0.0:53', nameserver: ['https://example.invalid/dns']}}\n", base());
        let (output, _, _, _, _) = controlled_config(input.as_bytes(), false).unwrap();
        let value: serde_yaml::Value = serde_yaml::from_str(&output).unwrap();
        assert_eq!(value["tun"]["device"].as_str(), Some(DEVICE));
        assert_eq!(value["tun"]["auto-route"].as_bool(), Some(false));
        assert_eq!(value["tun"]["auto-redirect"].as_bool(), Some(false));
        assert!(value["tun"]["inet6-address"]
            .as_sequence()
            .unwrap()
            .is_empty());
        assert_eq!(value["routing-mark"].as_u64(), Some(MARK as u64));
        assert_eq!(value["allow-lan"].as_bool(), Some(false));
        assert_eq!(value["profile"]["store-selected"].as_bool(), Some(true));
        assert!(value["listeners"].is_null() && value["iptables"].is_null());
        assert!(value["dns"]["listen"].is_null());
        assert_eq!(
            value["dns"]["nameserver"][0].as_str(),
            Some("https://example.invalid/dns")
        );
    }
    #[test]
    fn config_rejects_unsafe_provider_and_controller_inputs() {
        assert!(controlled_config(
            format!("{}rule-providers: {{bad: {{path: ../outside}}}}", base()).as_bytes(),
            false
        )
        .is_err());
        assert!(
            controlled_config(base().replace("127.0.0.1", "0.0.0.0").as_bytes(), false).is_err()
        );
        assert!(controlled_config(base().replace("19080", "80").as_bytes(), false).is_err());
        assert!(controlled_config(
            base()
                .replace("abcdef0123456789abcdef", "bad secret with spaces")
                .as_bytes(),
            false
        )
        .is_err());
    }
    #[test]
    fn network_check_without_a_core_returns_an_error_before_inspecting_routes() {
        // This unstarted fixture owns no resources. Skip Drop, which consults
        // the real runtime directory to clean up a production session.
        let session = std::mem::ManuallyDrop::new(Session::new());
        assert_eq!(
            session.verify_network().unwrap_err(),
            "TUN 会话缺少内核进程句柄"
        );
    }

    #[test]
    fn refuses_occupied_listener_ports_before_starting_core() {
        let occupied = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = occupied.local_addr().unwrap();
        assert!(ensure_ports_available(&[address]).is_err());
        drop(occupied);
        assert!(ensure_ports_available(&[address]).is_ok());
        assert!(ensure_ports_available(&[address, address]).is_err());
    }
    #[test]
    fn copies_desktop_geo_filenames_on_case_sensitive_filesystems() {
        let base = test_dir();
        let session = test_dir();
        for name in ["geosite.dat", "geoip.metadb", "GeoSite.dat"] {
            fs::write(base.join(name), name.as_bytes()).unwrap();
        }
        copy_assets(&base, &session, Vec::new()).unwrap();
        for name in ["geosite.dat", "geoip.metadb", "GeoSite.dat"] {
            assert_eq!(fs::read(session.join(name)).unwrap(), name.as_bytes());
        }
        fs::remove_dir_all(base).unwrap();
        fs::remove_dir_all(session).unwrap();
    }
    #[test]
    fn cache_writes_survive_session_removal_and_restarts() {
        let base = test_dir();
        let session = test_dir();
        let target = base.join("cache.db");
        fs::write(&target, b"existing selection").unwrap();
        let inode = fs::metadata(&target).unwrap().ino();
        link_cache(&base, &session).unwrap();
        verify_cache_link(&session).unwrap();
        let cached = session.join("cache.db");
        assert_eq!(fs::metadata(&cached).unwrap().ino(), inode);
        assert_eq!(fs::read(&cached).unwrap(), b"existing selection");
        fs::write(&cached, b"new selection").unwrap();
        fs::remove_dir_all(&session).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new selection");
        fs::create_dir(&session).unwrap();
        link_cache(&base, &session).unwrap();
        assert_eq!(fs::read(&cached).unwrap(), b"new selection");
        assert_eq!(fs::metadata(&cached).unwrap().ino(), inode);
        fs::remove_dir_all(base).unwrap();
        fs::remove_dir_all(session).unwrap();
    }
    #[test]
    fn cache_rejects_symlink_targets_locks_and_core_replacements() {
        let base = test_dir();
        let session = test_dir();
        let target = base.join("cache.db");
        let unrelated = base.join("unrelated");
        fs::write(&unrelated, b"untouched").unwrap();
        std::os::unix::fs::symlink(&unrelated, &target).unwrap();
        assert!(link_cache(&base, &session).is_err());
        assert_eq!(fs::read(&unrelated).unwrap(), b"untouched");
        fs::remove_file(&target).unwrap();
        let lock = File::create(&target).unwrap();
        assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
        assert!(link_cache(&base, &session).is_err());
        drop(lock);
        link_cache(&base, &session).unwrap();
        assert!(!cache_ready(&session).unwrap());
        let core_lock = File::open(&target).unwrap();
        assert_eq!(unsafe { libc::flock(core_lock.as_raw_fd(), libc::LOCK_EX) }, 0);
        assert!(cache_ready(&session).unwrap());
        drop(core_lock);
        assert!(!cache_ready(&session).unwrap());
        fs::remove_file(session.join("cache.db")).unwrap();
        fs::write(session.join("cache.db"), b"core rebuilt a corrupt DB").unwrap();
        assert!(verify_cache_link(&session).is_err());
        fs::remove_dir_all(base).unwrap();
        fs::remove_dir_all(session).unwrap();
    }
    #[test]
    fn resource_ledger_cannot_request_arbitrary_routes_or_owners() {
        let valid = json!({"version":1,"uid":uid(),"ifindex":7,"resources":["4:route:0","4:rule"],"pending":null});
        assert!(Journal::decode(valid.to_string().as_bytes()).is_ok());
        let mut invalid = valid.clone();
        invalid["uid"] = json!(uid().wrapping_add(1));
        assert!(Journal::decode(invalid.to_string().as_bytes()).is_err());
        let mut invalid = valid.clone();
        invalid["resources"] = json!(["4:route:999"]);
        assert!(Journal::decode(invalid.to_string().as_bytes()).is_err());
        for resource in Resource::all() {
            let args = resource.args("del");
            assert!(!args.iter().any(|arg| arg == "flush"));
            assert!(args.contains(&TABLE.to_string()) && args.contains(&PROTOCOL.to_string()));
        }
        let own = json!({"src":"all","not":null,"fwmark":format!("0x{MARK:x}"),"priority":PRIORITY,"table":TABLE.to_string(),"protocol":PROTOCOL.to_string()});
        assert!(Resource::Rule(4).matches(&own));
        let mut foreign = own.clone();
        foreign["iif"] = json!("wg-work");
        assert!(!Resource::Rule(4).matches(&foreign));
        let mut foreign = own;
        foreign["table"] = json!(52);
        assert!(!Resource::Rule(4).matches(&foreign));
    }
    #[test]
    fn wireguard_journal_keeps_guard_ahead_of_capture_and_rejects_unknown_resources() {
        let plan = WireguardPlan {
            device: "wg-test".into(),
            ifindex: 12,
            prefixes: vec!["198.51.100.2/32".into(), "2001:db8:4::2/128".into()],
            endpoints: vec!["198.18.0.2".parse().unwrap()],
        };
        let resources = Resource::for_session(Some(&plan));
        assert_eq!(resources.last(), Some(&Resource::WireguardCapture(6)),);
        let ids: Vec<_> = resources.iter().map(Resource::id).collect();
        let record =
            json!({"version":2, "uid":uid(), "ifindex":7, "resources":ids, "pending":null});
        assert_eq!(
            Journal::decode(record.to_string().as_bytes())
                .unwrap()
                .resources,
            resources
        );
        for bad in [
            "wg-route|../../eth0|12|198.51.100.2/32",
            "wg-route|wg-test|0|198.51.100.2/32",
            "wg-block|0.0.0.0/0",
            "wg-block|198.51.100.3/24",
            "wg-endpoint|127.0.0.1",
            "6:wg-capture|other-table",
        ] {
            assert!(Resource::parse(bad, 2).is_none(), "{bad}");
        }
        assert!(Resource::parse("4:wg-rule", 1).is_none());
        let (yaml, _, _, _, _) = controlled_config(base().as_bytes(), true).unwrap();
        let config: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        assert!(config["tun"]["dns-hijack"]
            .as_sequence()
            .unwrap()
            .is_empty());
        assert_eq!(config["dns"]["enhanced-mode"].as_str(), Some("redir-host"));
    }

    #[test]
    fn failed_add_never_records_another_owners_resource() {
        let old = Resource::Route(4, 0);
        let candidate = Resource::Rule(4);
        let mut journal = Journal {
            ifindex: 7,
            resources: vec![old.clone()],
            pending: None,
        };
        let mut saved = Vec::new();
        let result = journal.install_one(
            candidate.clone(),
            || Ok(false),
            || Err("EEXIST".into()),
            |state| {
                saved.push((state.resources.clone(), state.pending.clone()));
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(journal.resources.as_slice(), std::slice::from_ref(&old));
        assert_eq!(journal.pending, None);
        assert_eq!(saved.last(), Some(&(vec![old], None)));
        let result = journal.install_one(
            candidate,
            || Ok(true),
            || panic!("must not add an existing resource"),
            |_| panic!("must not record an existing resource"),
        );
        assert!(result.is_err());
    }
    #[test]
    fn exact_resource_presence_rejects_removed_replaced_or_duplicate_routes() {
        let route = Resource::Route(6, 0);
        let expected = json!({"dst":"default","dev":DEVICE,"table":TABLE,
            "protocol":PROTOCOL.to_string(),"metric":1024,"pref":"medium","flags":[]});
        assert!(route.exists_in(std::slice::from_ref(&expected)).unwrap());
        assert!(!route.exists_in(&[]).unwrap());
        let mut replaced = expected.clone();
        replaced["dev"] = json!("wg-work");
        assert!(!route.exists_in(&[replaced]).unwrap());
        let mut replaced = expected.clone();
        replaced["protocol"] = json!(99);
        assert!(!route.exists_in(&[replaced]).unwrap());
        let mut replaced = expected.clone();
        replaced["gateway"] = json!("2001:db8::1");
        assert!(!route.exists_in(&[replaced]).unwrap());
        assert!(route.exists_in(&[expected.clone(), expected]).is_err());
    }
    #[test]
    fn matches_iproute2_host_and_ipv6_throw_output() {
        // Captured in an isolated user/network namespace with iproute2.
        let v4 = json!({"type":"throw","dst":"255.255.255.255","table":"20220",
            "protocol":"242","flags":[]});
        let v6 = json!({"type":"throw","dst":"::1","dev":"lo","table":"20220",
            "protocol":"242","metric":1024,"flags":[],"pref":"medium"});
        assert!(Resource::Route(4, 8).exists_in(&[v4]).unwrap());
        assert!(Resource::Route(6, 1)
            .exists_in(std::slice::from_ref(&v6))
            .unwrap());
        let mut foreign = v6;
        foreign["dev"] = json!("wg-work");
        assert!(!Resource::Route(6, 1).exists_in(&[foreign]).unwrap());
    }
    #[test]
    fn socket_identity_and_partial_frames_are_checked_without_network_changes() {
        let (mut reader, mut writer) = UnixStream::pair().unwrap();
        reader
            .set_read_timeout(Some(Duration::from_millis(10)))
            .unwrap();
        assert_eq!(peer_uid(&reader).unwrap(), uid());
        let mut buffer = Vec::new();
        writer.write_all(b"{\"cmd\":").unwrap();
        assert!(read_frame(&mut reader, &mut buffer).unwrap().is_none());
        writer.write_all(b"\"status\"}\n").unwrap();
        assert_eq!(
            read_frame(&mut reader, &mut buffer).unwrap().unwrap()["cmd"],
            "status"
        );
        writer.write_all(&vec![b'x'; FRAME_LIMIT + 1]).unwrap();
        assert!(read_frame(&mut reader, &mut buffer).is_err());
    }
}
