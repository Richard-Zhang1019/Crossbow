//! Read-only conflict checks shared by the desktop and Linux service.
use serde_json::Value;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

pub const DEVICE: &str = "Crossbow";
pub const TABLE: u32 = 20220;
pub const PRIORITY: u32 = 12020;
pub const MARK: u32 = 49451;
pub const PROTOCOL: u32 = 242;
pub const WIREGUARD_TABLE: u32 = 20221;
pub const WIREGUARD_PRIORITY: u32 = 12019;
const MAX_WIREGUARD_PREFIXES: usize = 64;
const MAX_WIREGUARD_PEERS: usize = 16;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WireguardPlan {
    pub device: String,
    pub ifindex: u32,
    pub prefixes: Vec<String>,
    pub endpoints: Vec<IpAddr>,
}

pub fn ip(args: &[&str]) -> Result<String, String> {
    let program = ["/usr/sbin/ip", "/usr/bin/ip", "/sbin/ip"]
        .into_iter()
        .find(|path| Path::new(path).is_file())
        .ok_or("需要安装 iproute2 才能检查 Linux 网络状态")?;
    let output = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| format!("读取网络状态失败：{e}"))?;
    if !output.status.success() {
        return Err(format!(
            "ip {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| e.to_string())
}

fn rows(args: &[&str]) -> Result<Vec<Value>, String> {
    serde_json::from_str(&ip(args)?).map_err(|e| format!("无法解析网络状态：{e}"))
}

pub fn routing_rows(family: u8, rules: bool) -> Result<Vec<Value>, String> {
    let family = format!("-{family}");
    let args = if rules {
        vec!["-N", "-j", &family, "rule", "show"]
    } else {
        vec!["-N", "-j", &family, "route", "show", "table", "all"]
    };
    let mut result = rows(&args)?;
    // Ignore administrator display aliases for our reserved table and protocol.
    // Keep the standard names used by the shared parser for built-in values.
    for row in &mut result {
        for (field, aliases) in [
            (
                "table",
                &[("255", "local"), ("254", "main"), ("253", "default")][..],
            ),
            ("protocol", &[("2", "kernel")][..]),
            (
                "type",
                &[
                    ("1", "unicast"),
                    ("2", "local"),
                    ("3", "broadcast"),
                    ("5", "multicast"),
                    ("7", "unreachable"),
                    ("9", "throw"),
                ][..],
            ),
        ] {
            if let Some((_, name)) = aliases.iter().find(|(value, _)| row[field] == *value) {
                row[field] = (*name).into();
            }
        }
    }
    Ok(result)
}

pub fn device_index() -> Result<Option<u32>, String> {
    interface_index(DEVICE)
}

pub fn interface_index(name: &str) -> Result<Option<u32>, String> {
    Ok(rows(&["-j", "link", "show"])?
        .iter()
        .find(|link| link["ifname"] == name)
        .and_then(|link| link["ifindex"].as_u64())
        .and_then(|index| u32::try_from(index).ok()))
}

fn number(value: &Value, expected: u32) -> bool {
    value.as_u64() == Some(expected as u64)
        || value.as_str().is_some_and(|s| s == expected.to_string())
}

fn own_rule(rule: &Value) -> bool {
    number(&rule["priority"], PRIORITY)
        && number(&rule["table"], TABLE)
        && number(&rule["protocol"], PROTOCOL)
        && rule.get("not").is_some()
        && rule["fwmark"] == format!("0x{MARK:x}")
        && rule.get("fwmask").is_none_or(|mask| mask == "0xffffffff")
        && rule["src"] == "all"
        && rule.as_object().is_some_and(|m| {
            m.keys().all(|key| {
                matches!(
                    key.as_str(),
                    "priority" | "table" | "protocol" | "not" | "fwmark" | "fwmask" | "src"
                )
            })
        })
}

fn builtin_rule(rule: &Value) -> bool {
    let expected = match rule["priority"].as_u64() {
        Some(0) => "local",
        Some(32766) => "main",
        Some(32767) => "default",
        _ => return false,
    };
    rule["table"] == expected
        && rule["src"] == "all"
        && rule.as_object().is_some_and(|m| {
            m.keys()
                .all(|key| matches!(key.as_str(), "priority" | "table" | "src" | "protocol"))
        })
}

fn own_wireguard_rule(rule: &Value) -> bool {
    if !number(&rule["protocol"], PROTOCOL) || rule["src"] != "all" || rule["iif"] != "lo" {
        return false;
    }
    let guard =
        number(&rule["priority"], WIREGUARD_PRIORITY) && number(&rule["table"], WIREGUARD_TABLE);
    let capture = number(&rule["priority"], PRIORITY)
        && number(&rule["table"], TABLE)
        && matches!(rule["fwmark"].as_str(), Some("0" | "0x0"))
        && rule.get("fwmask").is_none_or(|mask| mask == "0xffffffff");
    (guard || capture)
        && rule.as_object().is_some_and(|fields| {
            fields.keys().all(|key| {
                matches!(
                    key.as_str(),
                    "priority" | "table" | "protocol" | "src" | "iif"
                ) || (capture && matches!(key.as_str(), "fwmark" | "fwmask"))
            })
        })
}

fn inspect(
    links: &[Value],
    rules: &[Value],
    routes: &[Value],
    own: Option<u32>,
    wireguard: Option<&WireguardPlan>,
) -> Vec<String> {
    let mut conflicts = Vec::new();
    for link in links {
        let name = link["ifname"].as_str().unwrap_or("unknown");
        let index = link["ifindex"].as_u64().map(|n| n as u32);
        if name == DEVICE && own.is_some() && index == own {
            continue;
        }
        if wireguard.is_some_and(|plan| plan.device == name && index == Some(plan.ifindex)) {
            continue;
        }
        let kind = link["linkinfo"]["info_kind"].as_str().unwrap_or("");
        let point_to_point = link["flags"]
            .as_array()
            .is_some_and(|flags| flags.iter().any(|flag| flag == "POINTOPOINT"));
        if name == DEVICE
            || point_to_point
            || matches!(
                kind,
                "tun" | "wireguard" | "gre" | "gretap" | "ipip" | "sit" | "ip6tnl" | "xfrm"
            )
        {
            conflicts.push(format!("已有隧道接口 {name}"));
        }
    }
    for rule in rules {
        let ours = own.is_some()
            && if wireguard.is_some() {
                own_wireguard_rule(rule)
            } else {
                own_rule(rule)
            };
        if !builtin_rule(rule) && !ours {
            conflicts.push(format!("已有策略路由（优先级 {}）", rule["priority"]));
        }
    }
    for route in routes {
        if number(&route["table"], TABLE) {
            let ours = own.is_some()
                && number(&route["protocol"], PROTOCOL)
                && (route["dev"] == DEVICE || route["type"] == "throw");
            if !ours {
                conflicts.push(format!("路由表 {TABLE} 已被占用"));
            }
        }
        if wireguard.is_some()
            && number(&route["table"], WIREGUARD_TABLE)
            && !(own.is_some() && number(&route["protocol"], PROTOCOL))
        {
            conflicts.push(format!("路由表 {WIREGUARD_TABLE} 已被占用"));
        }
        if matches!(
            route["dst"].as_str(),
            Some("0.0.0.0/1" | "128.0.0.0/1" | "::/1" | "8000::/1")
        ) && route["dev"] != DEVICE
        {
            conflicts.push("已有分段默认路由".into());
        }
    }
    conflicts.sort();
    conflicts.dedup();
    conflicts
}

pub fn valid_wireguard_device(device: &str) -> bool {
    !device.is_empty()
        && device.len() < libc::IFNAMSIZ
        && !matches!(device, "lo" | DEVICE | "." | "..")
        && device
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        && !device.starts_with('-')
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Prefix {
    address: IpAddr,
    bits: u8,
}

impl Prefix {
    fn parse(value: &str) -> Option<Self> {
        let (address, bits) = value
            .split_once('/')
            .map_or((value, None), |(address, bits)| (address, Some(bits)));
        let address: IpAddr = address.parse().ok()?;
        let width = if address.is_ipv4() { 32 } else { 128 };
        let bits = bits.map_or(Some(width), |bits| bits.parse::<u8>().ok())?;
        (bits <= width).then_some(Self { address, bits })
    }

    fn key(self, bits: u8) -> u128 {
        if bits == 0 {
            return 0;
        }
        match self.address {
            IpAddr::V4(addr) => u128::from(u32::from(addr) >> (32 - bits)),
            IpAddr::V6(addr) => u128::from(addr) >> (128 - bits),
        }
    }

    fn overlaps(self, other: Self) -> bool {
        let bits = self.bits.min(other.bits);
        self.address.is_ipv4() == other.address.is_ipv4() && self.key(bits) == other.key(bits)
    }

    fn is_host(self) -> bool {
        self.bits == if self.address.is_ipv4() { 32 } else { 128 }
    }

    fn label(self) -> String {
        format!("{}/{}", self.address, self.bits)
    }

    fn usable(self) -> bool {
        const RESERVED: &[&str] = &[
            "0.0.0.0/8",
            "127.0.0.0/8",
            "169.254.0.0/16",
            "224.0.0.0/3",
            "::/128",
            "::1/128",
            "::ffff:0:0/96",
            "fe80::/10",
            "ff00::/8",
        ];
        if self.bits <= 1 {
            return false;
        }
        let canonical = match self.address {
            IpAddr::V4(addr) => {
                u128::from(u32::from(addr)) == self.key(self.bits) << (32 - self.bits)
            }
            IpAddr::V6(addr) => u128::from(addr) == self.key(self.bits) << (128 - self.bits),
        };
        canonical
            && !RESERVED
                .iter()
                .filter_map(|range| Self::parse(range))
                .any(|range| self.overlaps(range))
    }
}

pub fn normalize_wireguard_prefix(value: &str) -> Option<String> {
    let prefix = Prefix::parse(value)?;
    prefix.usable().then(|| prefix.label())
}

fn main_route(route: &Value) -> bool {
    route.get("table").is_none() || route["table"] == "main" || number(&route["table"], 254)
}

fn local_route(route: &Value) -> bool {
    route["table"] == "local" || number(&route["table"], 255)
}

fn direct_route(route: &Value) -> bool {
    route.get("type").is_none_or(|kind| kind == "unicast")
        && route.as_object().is_some_and(|fields| {
            fields.keys().all(|key| {
                matches!(
                    key.as_str(),
                    "dst"
                        | "table"
                        | "dev"
                        | "protocol"
                        | "scope"
                        | "prefsrc"
                        | "metric"
                        | "flags"
                        | "pref"
                        | "type"
                )
            })
        })
        && route["flags"].as_array().is_none_or(Vec::is_empty)
}

fn wireguard_topology(links: &[Value], routes: &[Value]) -> Result<WireguardPlan, String> {
    let mut devices = links
        .iter()
        .filter(|link| link["linkinfo"]["info_kind"] == "wireguard");
    let link = devices
        .next()
        .filter(|link| {
            devices.next().is_none()
                && link["flags"]
                    .as_array()
                    .is_some_and(|flags| flags.iter().any(|flag| flag == "UP"))
        })
        .ok_or("WireGuard 共存需要一个已启用的原生 WireGuard 接口")?;
    let device = link["ifname"]
        .as_str()
        .filter(|device| valid_wireguard_device(device))
        .ok_or("无法核实 WireGuard 接口名称")?;
    let ifindex = link["ifindex"]
        .as_u64()
        .and_then(|index| u32::try_from(index).ok())
        .filter(|index| *index > 0)
        .ok_or("无法核实 WireGuard 接口身份")?;

    let interface_routes = || routes.iter().filter(|route| route["dev"] == device);
    let mut local_addresses = Vec::new();
    for route in interface_routes().filter(|route| local_route(route)) {
        let destination = route["dst"].as_str().and_then(Prefix::parse);
        match route["type"].as_str() {
            Some("local") if destination.is_some_and(Prefix::is_host) => {
                local_addresses.extend(destination);
            }
            Some("broadcast")
                if destination.is_some_and(|ip| ip.is_host() && ip.address.is_ipv4()) => {}
            Some("multicast") if route["dst"] == "ff00::/8" && route["protocol"] == "kernel" => {}
            _ => return Err("WireGuard 接口存在无法核实的本地路由".into()),
        }
    }

    let mut prefixes = Vec::new();
    for route in interface_routes().filter(|route| !local_route(route)) {
        if number(&route["table"], WIREGUARD_TABLE) {
            continue;
        }
        let prefix = route["dst"]
            .as_str()
            .and_then(Prefix::parse)
            .filter(|prefix| main_route(route) && direct_route(route) && prefix.usable())
            .ok_or("WireGuard 共存只支持主路由表中的直连分流前缀")?;
        // Linux installs an additional IPv6 /128 in main for an address that
        // the kernel already resolves through its earlier local rule.
        if route["protocol"] == "kernel" && local_addresses.contains(&prefix) {
            continue;
        }
        if let Some(source) = route.get("prefsrc") {
            let mut addresses = local_addresses.iter().filter(|address| {
                address.address.is_ipv4() == prefix.address.is_ipv4() && address.usable()
            });
            if !addresses
                .next()
                .is_some_and(|address| source == &address.address.to_string())
                || addresses.next().is_some()
            {
                return Err("WireGuard 分流路由使用无法保留的首选源地址".into());
            }
        }
        if prefixes.len() == MAX_WIREGUARD_PREFIXES {
            return Err("WireGuard 分流路由数量超出共存支持范围".into());
        }
        if prefixes.contains(&prefix) {
            return Err("WireGuard 分流前缀存在重复路由".into());
        }
        prefixes.push(prefix);
    }
    if prefixes.is_empty() {
        return Err("WireGuard 接口没有可核实的分流路由".into());
    }
    for route in routes
        .iter()
        .filter(|route| route["dev"] != device && (main_route(route) || local_route(route)))
    {
        if route["dst"]
            .as_str()
            .and_then(Prefix::parse)
            .is_some_and(|other| {
                other.bits != 0 && prefixes.iter().any(|prefix| prefix.overlaps(other))
            })
        {
            return Err("WireGuard 分流前缀与现有本地路由重叠".into());
        }
    }
    let mut prefixes: Vec<String> = prefixes.into_iter().map(Prefix::label).collect();
    prefixes.sort_unstable();
    Ok(WireguardPlan {
        device: device.to_string(),
        ifindex,
        prefixes,
        endpoints: Vec::new(),
    })
}

fn wg_output(device: &str, field: &str) -> Result<String, String> {
    let program = ["/usr/bin/wg", "/usr/sbin/wg", "/sbin/wg"]
        .into_iter()
        .find(|path| Path::new(path).is_file())
        .ok_or("WireGuard 共存需要系统安装 wireguard-tools")?;
    let output = Command::new(program)
        .args(["show", device, field])
        .env_clear()
        .env("LC_ALL", "C")
        .output()
        .map_err(|_| "无法读取 WireGuard 接口状态")?;
    if !output.status.success() {
        return Err("无法读取 WireGuard 接口状态".into());
    }
    String::from_utf8(output.stdout).map_err(|_| "无法解析 WireGuard 接口状态".into())
}

fn wireguard_endpoints(output: &str, protected: &[Prefix]) -> Result<Vec<IpAddr>, String> {
    let mut endpoints = Vec::new();
    for line in output.lines() {
        let endpoint = line
            .split_once('\t')
            .and_then(|(_, value)| value.parse::<SocketAddr>().ok())
            .filter(|endpoint| endpoint.port() != 0)
            .ok_or("WireGuard 共存需要可核实的固定对端地址")?;
        let address = endpoint.ip();
        let host = Prefix {
            address,
            bits: if address.is_ipv4() { 32 } else { 128 },
        };
        if !host.usable() || protected.iter().any(|prefix| prefix.overlaps(host)) {
            return Err("WireGuard 对端地址与保留网络或隧道路由冲突".into());
        }
        endpoints.push(address);
        if endpoints.len() > MAX_WIREGUARD_PEERS {
            return Err("WireGuard 对端数量超出共存支持范围".into());
        }
    }
    if endpoints.is_empty() {
        return Err("WireGuard 共存需要可核实的固定对端地址".into());
    }
    endpoints.sort_unstable();
    endpoints.dedup();
    Ok(endpoints)
}

fn verify_allowed_ips(output: &str, protected: &[Prefix]) -> Result<(), String> {
    let mut allowed = Vec::new();
    for line in output.lines() {
        let (_, prefixes) = line
            .split_once('\t')
            .ok_or("无法核实 WireGuard AllowedIPs")?;
        for value in prefixes
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|v| !v.is_empty())
        {
            let prefix = Prefix::parse(value)
                .filter(|prefix| prefix.usable())
                .ok_or("WireGuard 共存不支持全隧道或无法核实的 AllowedIPs")?;
            if allowed.len() == MAX_WIREGUARD_PREFIXES {
                return Err("WireGuard AllowedIPs 数量超出共存支持范围".into());
            }
            allowed.push(prefix);
        }
    }
    if protected.iter().any(|prefix| {
        !allowed
            .iter()
            .any(|entry| entry.bits <= prefix.bits && prefix.overlaps(*entry))
    }) {
        return Err("WireGuard 分流路由与 AllowedIPs 不匹配".into());
    }
    Ok(())
}

fn mihomo_version_probe(cmdline: &[u8]) -> bool {
    let Some(cmdline) = cmdline.strip_suffix(b"\0") else {
        return false;
    };
    let mut args = cmdline.split(|byte| *byte == 0);
    let Some(program) = args.next() else {
        return false;
    };
    Path::new(std::ffi::OsStr::from_bytes(program)).file_name()
        == Some(std::ffi::OsStr::new("mihomo"))
        && args.next() == Some(b"-v")
        && args.next().is_none()
}

fn proxy_exited(pid: u32, deadline: Instant) -> bool {
    // A process pidfd stays tied to the same PID and becomes readable only
    // after every thread exits. Never use a name or an empty cmdline as proof.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0u32) };
    if fd < 0 {
        return io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd as i32) };
    let mut pollfd = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let timeout = deadline
        .saturating_duration_since(Instant::now())
        .as_millis() as i32;
    let ready = unsafe { libc::poll(&mut pollfd, 1, timeout) };
    ready > 0 && pollfd.revents & (libc::POLLIN | libc::POLLHUP) != 0
}

fn proxy_processes(own_pids: &[u32], wireguard: bool) -> Vec<String> {
    let mut found = Vec::new();
    let our_net = std::fs::read_link("/proc/self/ns/net").ok();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return found;
    };
    let mut confirmation_deadline = None;
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        if own_pids.contains(&pid) {
            continue;
        }
        let path = entry.path();
        let Ok(name) = std::fs::read_to_string(path.join("comm")) else {
            continue;
        };
        let lower = name.trim().to_ascii_lowercase();
        if !(lower.contains("clash")
            || lower.contains("mihomo")
            || lower.contains("sing-box")
            || (wireguard && lower == "tailscaled"))
        {
            continue;
        }
        if lower == "mihomo" {
            match std::fs::read(path.join("cmdline")) {
                // Settings refreshes the version after toggling TUN. This
                // exact command prints the version and never starts a proxy.
                Ok(args)
                    if mihomo_version_probe(&args)
                        && std::fs::read_link(path.join("exe")).is_ok_and(|exe| {
                            exe.file_name().is_some_and(|name| name == "mihomo")
                        }) =>
                {
                    #[cfg(debug_assertions)]
                    eprintln!("Ignoring mihomo -v version probe (pid {pid})");
                    continue;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                _ => {}
            }
        }
        // Ignore processes proven to belong to another network namespace.
        let other_net = std::fs::read_link(path.join("ns/net")).ok();
        if our_net.is_some() && other_net.is_some() && our_net != other_net {
            continue;
        }
        // A read-only version command can exit while procfs metadata is
        // sampled. Bound the whole scan's exit confirmation to 25 ms.
        let deadline = *confirmation_deadline
            .get_or_insert_with(|| Instant::now() + Duration::from_millis(25));
        if proxy_exited(pid, deadline) {
            #[cfg(debug_assertions)]
            eprintln!("Ignoring exited proxy task {} (pid {pid})", name.trim());
            continue;
        }
        found.push(format!("已有代理进程 {}（pid {pid}）", name.trim()));
    }
    found
}

fn rp_filter_value(value: &str) -> Option<u8> {
    value.trim().parse::<u8>().ok().filter(|value| *value <= 2)
}

fn read_rp_filter(interface: &str) -> Result<u8, String> {
    // Interface names come from iproute2, but must remain one procfs component.
    if interface.is_empty() || matches!(interface, "." | "..") || interface.contains('/') {
        return Err("无法确认 IPv4 接口的反向路径检查参数：接口名无效，未接管网络".into());
    }
    let setting = format!("net.ipv4.conf.{interface}.rp_filter");
    let path = Path::new("/proc/sys/net/ipv4/conf")
        .join(interface)
        .join("rp_filter");
    let value = std::fs::read_to_string(path)
        .map_err(|error| format!("无法读取 {setting}：{error}；未接管网络"))?;
    rp_filter_value(&value).ok_or_else(|| format!("{setting} 的值无效，未接管网络"))
}

fn rp_filter_conflicts(
    addresses: &[Value],
    own_ifindex: Option<u32>,
    mut read: impl FnMut(&str) -> Result<u8, String>,
) -> Result<Vec<String>, String> {
    let mut interfaces = Vec::new();
    for link in addresses {
        let flags = link["flags"].as_array();
        if !flags.is_some_and(|flags| flags.iter().any(|flag| flag == "UP"))
            || flags.is_some_and(|flags| flags.iter().any(|flag| flag == "LOOPBACK"))
            || matches!(
                link["operstate"].as_str(),
                Some("DOWN" | "LOWERLAYERDOWN" | "DORMANT" | "NOTPRESENT")
            )
            || (link["ifname"] == DEVICE
                && own_ifindex.is_some()
                && link["ifindex"].as_u64() == own_ifindex.map(u64::from))
        {
            continue;
        }
        // RFC1918 addresses also have global scope: ordinary Wi-Fi/Ethernet
        // uplinks behind NAT need the same check as publicly addressed links.
        let ipv4 = link["addr_info"].as_array().is_some_and(|addresses| {
            addresses.iter().any(|address| {
                address["family"] == "inet"
                    && address["scope"] == "global"
                    && address["local"]
                        .as_str()
                        .and_then(|value| value.parse::<std::net::Ipv4Addr>().ok())
                        .is_some_and(|address| !address.is_loopback() && !address.is_unspecified())
            })
        });
        if ipv4 {
            interfaces.push(
                link["ifname"]
                    .as_str()
                    .ok_or("无法确认 IPv4 出口接口，未接管网络")?,
            );
        }
    }
    if interfaces.is_empty() {
        return Ok(Vec::new());
    }
    let all = read("all")?;
    let mut conflicts = Vec::new();
    for interface in interfaces {
        let value = read(interface)?;
        // Linux uses the numeric max, not a boolean OR: 1 + 2 is loose mode.
        // https://docs.kernel.org/networking/ip-sysctl.html#rp-filter-integer
        if all.max(value) == 1 {
            conflicts.push(format!("IPv4 接口 {interface} 的有效 rp_filter=1（all={all}，接口={value}）；严格反向路径检查会丢弃 TUN 的公网回包，请管理员检查网络策略"));
        }
    }
    Ok(conflicts)
}

fn check_network(
    own_ifindex: Option<u32>,
    own_pids: &[u32],
    enabled: bool,
    require_device: bool,
) -> Result<(Vec<String>, Option<WireguardPlan>), String> {
    let links = rows(&["-j", "-d", "link", "show"])?;
    let mut rules = routing_rows(4, true)?;
    rules.extend(routing_rows(6, true)?);
    let mut routes = routing_rows(4, false)?;
    routes.extend(routing_rows(6, false)?);
    let plan = if enabled
        && (require_device
            || own_ifindex.is_some()
            || links
                .iter()
                .any(|link| link["linkinfo"]["info_kind"] == "wireguard"))
    {
        Some(wireguard_topology(&links, &routes)?)
    } else {
        None
    };
    let mut result = inspect(&links, &rules, &routes, own_ifindex, plan.as_ref());
    result.extend(rp_filter_conflicts(
        &rows(&["-j", "-4", "address", "show", "up"])?,
        own_ifindex,
        read_rp_filter,
    )?);
    result.extend(proxy_processes(own_pids, enabled));
    Ok((result, plan))
}

fn ensure_no_conflicts(found: Vec<String>) -> Result<(), String> {
    if found.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "未接管网络：{}。请处理上述问题后重试；Crossbow 不会修改系统参数或停止其他代理和 VPN",
            found.join("；")
        ))
    }
}

pub fn conflicts(own_ifindex: Option<u32>, own_pids: &[u32]) -> Result<Vec<String>, String> {
    conflicts_with_wireguard(own_ifindex, own_pids, false)
}

pub fn conflicts_with_wireguard(
    own_ifindex: Option<u32>,
    own_pids: &[u32],
    enabled: bool,
) -> Result<Vec<String>, String> {
    Ok(check_network(own_ifindex, own_pids, enabled, false)?.0)
}

pub fn ensure_available(own_ifindex: Option<u32>, own_pids: &[u32]) -> Result<(), String> {
    ensure_available_with_wireguard(own_ifindex, own_pids, false)
}

pub fn ensure_available_with_wireguard(
    own_ifindex: Option<u32>,
    own_pids: &[u32],
    enabled: bool,
) -> Result<(), String> {
    ensure_no_conflicts(check_network(own_ifindex, own_pids, enabled, true)?.0)
}

pub fn wireguard_plan(own_ifindex: Option<u32>, own_pids: &[u32]) -> Result<WireguardPlan, String> {
    let (found, plan) = check_network(own_ifindex, own_pids, true, true)?;
    ensure_no_conflicts(found)?;
    let mut plan = plan.ok_or("无法核实 WireGuard 分流状态")?;
    let protected: Vec<Prefix> = plan
        .prefixes
        .iter()
        .filter_map(|prefix| Prefix::parse(prefix))
        .collect();
    if wg_output(&plan.device, "fwmark")?.trim() != "off" {
        return Err("WireGuard 共存首版不支持额外的路由标记".into());
    }
    verify_allowed_ips(&wg_output(&plan.device, "allowed-ips")?, &protected)?;
    plan.endpoints = wireguard_endpoints(&wg_output(&plan.device, "endpoints")?, &protected)?;
    for endpoint in &plan.endpoints {
        let family = if endpoint.is_ipv4() { "-4" } else { "-6" };
        // During an active session, the mihomo mark skips our public default
        // rule. Verified WireGuard endpoints never overlap the protected table.
        let path = rows(&[
            "-j",
            family,
            "route",
            "get",
            &endpoint.to_string(),
            "mark",
            &MARK.to_string(),
        ])
        .map_err(|_| "无法核实 WireGuard 对端的原有出口")?;
        if !matches!(path.as_slice(), [route] if
            route.get("type").is_none_or(|kind| kind == "unicast")
                && route["dev"].as_str().is_some_and(|name| name != plan.device && !matches!(name, "lo" | DEVICE)))
        {
            return Err("WireGuard 对端没有可确认的外部出口".into());
        }
    }
    if interface_index(&plan.device)? != Some(plan.ifindex) {
        return Err("WireGuard 接口在预检期间发生变化".into());
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn recognizes_only_the_mihomo_version_query() {
        assert!(mihomo_version_probe(b"/usr/lib/crossbow/mihomo\0-v\0"));
        for args in [
            b"mihomo\0".as_slice(),
            b"mihomo\0-d\0/run/crossbow-tun/session\0",
            b"mihomo\0-v\0-f\0/config.yaml\0",
            b"clash\0-v\0",
            b"mihomo\0-v",
            b"",
        ] {
            assert!(!mihomo_version_probe(args));
        }
    }

    #[test]
    fn confirms_exit_without_exempting_a_live_process() {
        let mut live = Command::new("/bin/sleep").arg("5").spawn().unwrap();
        let conflicts = !proxy_exited(live.id(), Instant::now());
        live.kill().unwrap();
        let exited = proxy_exited(live.id(), Instant::now() + Duration::from_secs(1));
        live.wait().unwrap();
        assert!(conflicts);
        assert!(exited);
    }

    #[test]
    fn detects_vpn_policy_routes_and_occupied_resources() {
        let links =
            vec![json!({"ifname":"wg-work","ifindex":4,"linkinfo":{"info_kind":"wireguard"}})];
        let rules = vec![json!({"priority":5270,"src":"all","table":"52"})];
        assert_eq!(inspect(&links, &rules, &[], None, None).len(), 2);
        let own = json!({"ifname":DEVICE,"ifindex":9,"flags":["POINTOPOINT"]});
        assert!(!inspect(std::slice::from_ref(&own), &[], &[], None, None).is_empty());
        assert!(inspect(&[own], &[], &[], Some(9), None).is_empty());
        assert!(!inspect(
            &[],
            &[],
            &[json!({"table":TABLE,"dst":"default"})],
            None,
            None
        )
        .is_empty());
    }
    #[test]
    fn only_exact_own_and_standard_rules_are_ignored() {
        let mut own = json!({"priority":PRIORITY,"src":"all","table":TABLE.to_string(),
            "not":null,"fwmark":format!("0x{MARK:x}"),"protocol":PROTOCOL.to_string()});
        assert!(own_rule(&own));
        assert!(!inspect(&[], std::slice::from_ref(&own), &[], None, None).is_empty());
        assert!(inspect(&[], std::slice::from_ref(&own), &[], Some(9), None).is_empty());
        own["table"] = json!(52);
        assert!(!own_rule(&own));
        assert!(builtin_rule(
            &json!({"priority":32766,"src":"all","table":"main"})
        ));
        assert!(!builtin_rule(
            &json!({"priority":32766,"src":"all","table":"main","fwmark":"0x1"})
        ));
    }

    fn wireguard_link() -> Value {
        json!({"ifname":"wg-work","ifindex":4,"flags":["UP","POINTOPOINT"],"linkinfo":{"info_kind":"wireguard"}})
    }

    #[test]
    fn wireguard_topology_allows_only_one_native_split_tunnel() {
        let link = wireguard_link();
        let links = std::slice::from_ref(&link);
        let routes = vec![
            json!({"dst":"10.1.2.3","dev":"wg-work","scope":"link"}),
            json!({"dst":"fd00:2::/64","dev":"wg-work","metric":1024,"pref":"medium"}),
            json!({"dst":"10.1.2.4","dev":"wg-work","table":"local","type":"local"}),
            json!({"dst":"fd00:3::2","dev":"wg-work","protocol":"kernel","metric":256,"pref":"medium"}),
            json!({"dst":"fd00:3::2","dev":"wg-work","table":"local","type":"local"}),
            json!({"dst":"ff00::/8","dev":"wg-work","table":"local","type":"multicast","protocol":"kernel"}),
            json!({"dst":"default","dev":"eth0","gateway":"192.168.1.1"}),
        ];
        let plan = wireguard_topology(links, &routes).unwrap();
        assert_eq!(plan.prefixes, ["10.1.2.3/32", "fd00:2::/64"]);
        assert_eq!(plan.ifindex, 4);
        assert!(inspect(links, &[], &routes, None, Some(&plan)).is_empty());
        assert!(wireguard_topology(&[], &routes).is_err());
        assert!(wireguard_topology(&[link.clone(), link.clone()], &routes).is_err());
        for route in [
            json!({"dst":"default","dev":"wg-work"}),
            json!({"dst":"128.0.0.0/1","dev":"wg-work"}),
            json!({"dst":"10.1.2.3","dev":"wg-work","gateway":"192.168.1.1"}),
            json!({"dst":"10.1.2.3","dev":"wg-work","table":52}),
            json!({"dst":"10.0.0.0/8","dev":"eth0"}),
            json!({"dst":"fd00:2::4","dev":"eth0","table":"local"}),
            json!({"dst":"default","dev":"wg-work","table":"local","type":"local"}),
            json!({"dst":"10.2.0.0/24","dev":"wg-work","prefsrc":"10.1.2.5"}),
        ] {
            let mut unsupported = routes.clone();
            unsupported.push(route);
            assert!(wireguard_topology(links, &unsupported).is_err());
        }
    }

    #[test]
    fn only_exact_active_wireguard_rules_and_reserved_resources_are_admitted() {
        let link = wireguard_link();
        let links = std::slice::from_ref(&link);
        let routes = vec![json!({"dst":"10.1.2.3","dev":"wg-work"})];
        let plan = wireguard_topology(links, &routes).unwrap();
        let guard = json!({"priority":WIREGUARD_PRIORITY,"src":"all","table":WIREGUARD_TABLE,"iif":"lo","protocol":PROTOCOL});
        let capture = json!({"priority":PRIORITY,"src":"all","table":TABLE,"iif":"lo","protocol":PROTOCOL,"fwmark":"0"});
        let rules = [guard, capture];
        assert!(inspect(links, &rules, &routes, None, Some(&plan)).len() == 2);
        assert!(inspect(links, &rules, &routes, Some(9), Some(&plan)).is_empty());
        let mut foreign_rules = rules.clone();
        foreign_rules[1]["fwmask"] = json!("0xff");
        assert_eq!(
            inspect(links, &foreign_rules, &routes, Some(9), Some(&plan)).len(),
            1
        );
        let mut foreign_routes = routes.clone();
        foreign_routes.push(json!({"dst":"10.1.2.3","table":WIREGUARD_TABLE,"protocol":"static","type":"unreachable"}));
        assert_eq!(
            inspect(links, &rules, &foreign_routes, Some(9), Some(&plan)).len(),
            1
        );
        let other_vpn = json!({"ifname":"tailscale0","ifindex":5,"flags":["UP","POINTOPOINT"],"linkinfo":{"info_kind":"tun"}});
        assert_eq!(
            inspect(&[link, other_vpn], &rules, &routes, Some(9), Some(&plan)).len(),
            1
        );
    }

    #[test]
    fn fixed_endpoints_and_allowed_ips_are_bounded_and_separate_from_protected_routes() {
        let protected = [
            Prefix::parse("10.1.2.3/32").unwrap(),
            Prefix::parse("fd00:2::/64").unwrap(),
        ];
        let endpoints = wireguard_endpoints(
            "peer-v4\t192.0.2.2:51820\npeer-v6\t[2001:db8::2]:51820\n",
            &protected,
        )
        .unwrap();
        assert_eq!(
            endpoints,
            [
                "192.0.2.2".parse::<IpAddr>().unwrap(),
                "2001:db8::2".parse::<IpAddr>().unwrap()
            ]
        );
        for output in [
            "",
            "peer\t(none)",
            "peer\t10.1.2.3:51820",
            "peer\t[fd00:2::4]:51820",
            "peer\t127.0.0.1:51820",
            "peer\t192.0.2.2:0",
        ] {
            assert!(wireguard_endpoints(output, &protected).is_err());
        }
        assert!(verify_allowed_ips("peer\t10.1.2.3/32 fd00:2::/64\n", &protected).is_ok());
        for output in [
            "peer\t0.0.0.0/0",
            "peer\t::/1",
            "peer\t10.1.2.3/32",
            "peer\t10.1.2.3/32 fd00:2::1/128",
            "peer\t(none)",
        ] {
            assert!(verify_allowed_ips(output, &protected).is_err());
        }
        assert_eq!(
            normalize_wireguard_prefix("fd00:2::7").as_deref(),
            Some("fd00:2::7/128")
        );
        assert!(normalize_wireguard_prefix("10.1.2.3/24").is_none());
    }

    fn ipv4_link(name: &str, index: u32) -> Value {
        json!({"ifname":name,"ifindex":index,"flags":["UP","LOWER_UP"],"operstate":"UP",
            "addr_info":[{"family":"inet","scope":"global","local":"192.168.1.2"}]})
    }

    #[test]
    fn rp_filter_uses_numeric_max_for_all_and_interface() {
        for all in 0..=2 {
            for interface in 0..=2 {
                let found = rp_filter_conflicts(&[ipv4_link("eth0", 2)], None, |name| {
                    Ok(if name == "all" { all } else { interface })
                })
                .unwrap();
                assert_eq!(
                    !found.is_empty(),
                    all.max(interface) == 1,
                    "all={all}, interface={interface}"
                );
            }
        }
    }

    #[test]
    fn rp_filter_checks_only_usable_ipv4_links_and_excludes_exact_own_tun() {
        let mut loopback = ipv4_link("lo", 1);
        loopback["flags"] = json!(["UP", "LOOPBACK"]);
        let mut down = ipv4_link("eth1", 3);
        down["operstate"] = json!("DOWN");
        let mut ipv6 = ipv4_link("eth2", 4);
        ipv6["addr_info"] = json!([{"family":"inet6","scope":"global","local":"2001:db8::1"}]);
        let mut link_local = ipv4_link("eth3", 5);
        link_local["addr_info"] = json!([{"family":"inet","scope":"link","local":"169.254.1.1"}]);
        let own = ipv4_link(DEVICE, 6);
        assert!(rp_filter_conflicts(
            &[loopback, down, ipv6, link_local, own.clone()],
            Some(6),
            |_| panic!(
                "inactive, local-only, IPv6-only and own TUN links need no IPv4 sysctl read"
            )
        )
        .unwrap()
        .is_empty());
        assert_eq!(
            rp_filter_conflicts(&[own], Some(7), |_| Ok(1))
                .unwrap()
                .len(),
            1
        );
        let mut unknown = ipv4_link("ppp-test", 8);
        unknown["operstate"] = json!("UNKNOWN");
        assert_eq!(
            rp_filter_conflicts(&[unknown], None, |_| Ok(1))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn rp_filter_unknown_values_or_unreadable_settings_fail_closed() {
        for value in ["", "-1", "3", "strict", "1 2"] {
            assert!(rp_filter_value(value).is_none());
        }
        assert_eq!(rp_filter_value("2\n"), Some(2));
        for missing in ["all", "eth0"] {
            let error = rp_filter_conflicts(&[ipv4_link("eth0", 2)], None, |name| {
                if name == missing {
                    Err("unreadable setting".into())
                } else {
                    Ok(0)
                }
            })
            .unwrap_err();
            assert_eq!(error, "unreadable setting");
        }
    }
}
