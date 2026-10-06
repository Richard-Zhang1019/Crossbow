//! 系统代理管理：按平台后端设置/还原系统级代理，带崩溃兜底 journal。
//!
//! 兜底思路（DESIGN §4.4 状态一致性守护）：开启成功后把状态写入 journal
//! 文件；正常关闭/退出时清除。若上次会话异常退出（journal 残留），本次启动
//! 先执行一次全量还原——保证「任何路径退出都不留脏系统代理」。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// 默认绕过列表（本机/局域网直连）。
const BYPASS_DOMAINS: &[&str] = &[
    "127.0.0.1",
    "localhost",
    "192.168.0.0/16",
    "10.0.0.0/8",
    "172.16.0.0/12",
    "*.local",
    "*.crashlytics.com",
];

pub trait SysProxyBackend: Send + Sync {
    /// 枚举可设置代理的网络服务。
    fn list_services(&self) -> Result<Vec<String>, String>;
    /// 为单个服务开启代理（HTTP/HTTPS/SOCKS → 127.0.0.1:port + bypass）。
    fn enable_service(&self, service: &str, port: u16) -> Result<(), String>;
    /// 为单个服务全量关闭三类代理（无快照时的兜底路径）。
    fn disable_service(&self, service: &str) -> Result<(), String>;
    /// 读取单个服务当前的三类代理设置（开启前快照用）。
    fn service_snapshot(&self, service: &str) -> Result<ServiceSnapshot, String>;
    /// 按快照还原单个服务设置。
    fn restore_service(&self, service: &str, snap: &ServiceSnapshot) -> Result<(), String>;
}

/// macOS `networksetup` 实现。
#[cfg(target_os = "macos")]
pub struct NetworkSetup;

#[cfg(target_os = "macos")]
impl NetworkSetup {
    fn run(args: &[&str]) -> Result<String, String> {
        let out = std::process::Command::new("networksetup")
            .args(args)
            .output()
            .map_err(|e| format!("run networksetup: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "networksetup {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }
}

#[cfg(target_os = "macos")]
impl SysProxyBackend for NetworkSetup {
    fn list_services(&self) -> Result<Vec<String>, String> {
        let out = Self::run(&["-listallnetworkservices"])?;
        Ok(parse_services(&out))
    }

    fn enable_service(&self, service: &str, port: u16) -> Result<(), String> {
        let port = port.to_string();
        for setter in [
            "-setwebproxy",
            "-setsecurewebproxy",
            "-setsocksfirewallproxy",
        ] {
            Self::run(&[setter, service, "127.0.0.1", &port])?;
        }
        let mut args = vec!["-setproxybypassdomains", service];
        args.extend(BYPASS_DOMAINS.iter().copied());
        Self::run(&args)?;
        Ok(())
    }

    fn disable_service(&self, service: &str) -> Result<(), String> {
        for setter in [
            "-setwebproxy",
            "-setsecurewebproxy",
            "-setsocksfirewallproxy",
        ] {
            Self::run(&[setter, service, "off"])?;
        }
        Ok(())
    }

    fn service_snapshot(&self, service: &str) -> Result<ServiceSnapshot, String> {
        Ok(ServiceSnapshot {
            web: Self::run(&["-getwebproxy", service])
                .ok()
                .as_deref()
                .and_then(parse_endpoint),
            secure: Self::run(&["-getsecurewebproxy", service])
                .ok()
                .as_deref()
                .and_then(parse_endpoint),
            socks: Self::run(&["-getsocksfirewallproxy", service])
                .ok()
                .as_deref()
                .and_then(parse_endpoint),
        })
    }

    fn restore_service(&self, service: &str, snap: &ServiceSnapshot) -> Result<(), String> {
        let apply = |setter: &str, ep: Option<&ProxyEndpoint>| -> Result<(), String> {
            match ep {
                Some(ep) => {
                    Self::run(&[setter, service, &ep.host, &ep.port.to_string()])?;
                    Ok(())
                }
                None => Self::run(&[setter, service, "off"]).map(|_| ()),
            }
        };
        apply("-setwebproxy", snap.web.as_ref())?;
        apply("-setsecurewebproxy", snap.secure.as_ref())?;
        apply("-setsocksfirewallproxy", snap.socks.as_ref())?;
        Ok(())
    }
}

/// 非 macOS 平台桩：M1.5 接入 Windows 注册表实现；保持编译通过。
#[cfg(not(target_os = "macos"))]
pub struct UnsupportedBackend;

#[cfg(not(target_os = "macos"))]
impl SysProxyBackend for UnsupportedBackend {
    fn list_services(&self) -> Result<Vec<String>, String> {
        Err("sysproxy: platform backend not implemented yet".into())
    }
    fn enable_service(&self, _: &str, _: u16) -> Result<(), String> {
        Err("sysproxy: platform backend not implemented yet".into())
    }
    fn disable_service(&self, _: &str) -> Result<(), String> {
        Err("sysproxy: platform backend not implemented yet".into())
    }
    fn service_snapshot(&self, _: &str) -> Result<ServiceSnapshot, String> {
        Err("sysproxy: platform backend not implemented yet".into())
    }
    fn restore_service(&self, _: &str, _: &ServiceSnapshot) -> Result<(), String> {
        Err("sysproxy: platform backend not implemented yet".into())
    }
}

#[cfg(target_os = "macos")]
pub(crate) type PlatformBackend = NetworkSetup;
#[cfg(not(target_os = "macos"))]
pub(crate) type PlatformBackend = UnsupportedBackend;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct SysProxyStatus {
    pub enabled: bool,
    pub port: u16,
}

/// 单个代理槽位的原设置（HTTP/HTTPS/SOCKS 各一）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyEndpoint {
    pub host: String,
    pub port: u16,
}

/// 一个网络服务的代理设置快照；None = 该槽位原本关闭。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceSnapshot {
    pub web: Option<ProxyEndpoint>,
    pub secure: Option<ProxyEndpoint>,
    pub socks: Option<ProxyEndpoint>,
}

/// journal v2：开启前的原设置快照，disable 时优先还原快照（保住第三方客户端
/// 的配置），无快照才退回全量关闭。
#[derive(Debug, Serialize, Deserialize)]
struct Journal {
    port: u16,
    snapshot: BTreeMap<String, ServiceSnapshot>,
}

pub struct SysProxyManager<B: SysProxyBackend = PlatformBackend> {
    backend: B,
    /// Some(port) = 已开启。
    state: Mutex<Option<u16>>,
    journal_path: PathBuf,
}

impl<B: SysProxyBackend> SysProxyManager<B> {
    pub fn new(backend: B, journal_path: PathBuf) -> Self {
        Self {
            backend,
            state: Mutex::new(None),
            journal_path,
        }
    }

    /// 对所有网络服务开启代理。开启前先快照原设置（崩溃兜底与还原都靠它）；
    /// 逐服务尽力执行，全部失败才报错。
    pub fn enable(&self, port: u16) -> Result<(), String> {
        let services = self.backend.list_services()?;
        if services.is_empty() {
            return Err("no network services found".into());
        }
        // 先快照再动手
        let mut snapshot = BTreeMap::new();
        for svc in &services {
            if let Ok(snap) = self.backend.service_snapshot(svc) {
                snapshot.insert(svc.clone(), snap);
            }
        }
        let mut ok = 0;
        let mut last_err = String::new();
        for svc in &services {
            match self.backend.enable_service(svc, port) {
                Ok(()) => ok += 1,
                Err(e) => last_err = format!("{svc}: {e}"),
            }
        }
        if ok == 0 {
            return Err(format!("failed on all services; last error: {last_err}"));
        }
        *self.state.lock().unwrap() = Some(port);
        self.write_journal(port, &snapshot);
        Ok(())
    }

    /// 还原代理设置：优先按 journal 里的原设置快照逐服务恢复；无快照（老格式
    /// 或快照失败）退回全量关闭。幂等，用于退出兜底。
    pub fn disable(&self) {
        let journal = self.read_journal();
        if let Some(snapshot) = journal.map(|j| j.snapshot).filter(|s| !s.is_empty()) {
            for (svc, snap) in &snapshot {
                let _ = self.backend.restore_service(svc, snap);
            }
        } else if let Ok(services) = self.backend.list_services() {
            for svc in &services {
                let _ = self.backend.disable_service(svc);
            }
        }
        *self.state.lock().unwrap() = None;
        let _ = std::fs::remove_file(&self.journal_path);
    }

    /// 各服务当前生效的 HTTP/HTTPS 代理端点（含第三方设置的）。
    /// 诊断用：判断系统代理是否指向本应用端口、是否存在第三方冲突。
    pub fn external_endpoints(&self) -> Vec<(String, ProxyEndpoint)> {
        let mut out = Vec::new();
        if let Ok(services) = self.backend.list_services() {
            for svc in &services {
                if let Ok(snap) = self.backend.service_snapshot(svc) {
                    if let Some(ep) = snap.web {
                        out.push((svc.clone(), ep));
                    }
                    if let Some(ep) = snap.secure {
                        out.push((svc.clone(), ep));
                    }
                }
            }
        }
        out
    }

    pub fn status(&self) -> SysProxyStatus {
        match *self.state.lock().unwrap() {
            Some(port) => SysProxyStatus {
                enabled: true,
                port,
            },
            None => SysProxyStatus {
                enabled: false,
                port: 0,
            },
        }
    }

    /// 收养场景：轻量交接退出时系统代理保持开启（journal 留存），
    /// 本次启动若内核被成功收养，则从 journal 恢复内存状态而不是还原设置。
    /// 返回收养的端口。
    pub fn adopt_from_journal(&self) -> Option<u16> {
        let j = self.read_journal()?;
        if j.port == 0 {
            return None; // v1 兼容格式无端口信息，走 recover_stale 路径
        }
        *self.state.lock().unwrap() = Some(j.port);
        Some(j.port)
    }

    /// 启动时兜底：上次会话残留的开启状态 → 还原原设置。返回是否发生了还原。
    pub fn recover_stale(&self) -> bool {
        self.read_journal().is_some_and(|_| {
            self.disable();
            true
        })
    }

    fn read_journal(&self) -> Option<Journal> {
        let raw = std::fs::read_to_string(&self.journal_path).ok()?;
        // 兼容 v1（enabled+port，无 snapshot）：退化到全量关闭路径。
        if let Ok(j) = serde_json::from_str::<Journal>(&raw) {
            return Some(j);
        }
        serde_json::from_str::<serde_json::Value>(&raw)
            .ok()
            .map(|_| Journal {
                port: 0,
                snapshot: BTreeMap::new(),
            })
    }

    fn write_journal(&self, port: u16, snapshot: &BTreeMap<String, ServiceSnapshot>) {
        let j = Journal {
            port,
            snapshot: snapshot.clone(),
        };
        if let Some(parent) = self.journal_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(
            &self.journal_path,
            serde_json::to_vec(&j).unwrap_or_default(),
        );
    }
}

/// 解析 `-listallnetworkservices` 输出：跳过说明行，跳过禁用（`*`）服务。
fn parse_services(output: &str) -> Vec<String> {
    output
        .lines()
        .skip(1) // "An asterisk (*) denotes..."
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('*'))
        .map(str::to_string)
        .collect()
}

/// 解析 `-getwebproxy` 类输出为槽位设置；未启用返回 None。
fn parse_endpoint(output: &str) -> Option<ProxyEndpoint> {
    let mut enabled = false;
    let mut host = String::new();
    let mut port = 0u16;
    for line in output.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("Enabled:") {
            enabled = v.trim() == "Yes";
        } else if let Some(v) = line.strip_prefix("Server:") {
            host = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("Port:") {
            port = v.trim().parse().unwrap_or(0);
        }
    }
    (enabled && port > 0).then_some(ProxyEndpoint { host, port })
}

/// 解析 `-getwebproxy` 输出中的 Enabled 行。
#[cfg(test)]
fn parse_proxy_enabled(output: &str) -> bool {
    output.lines().any(|l| {
        let l = l.trim();
        l.starts_with("Enabled:") && l.ends_with("Yes")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Arc;

    #[test]
    fn parses_service_list_skipping_disabled() {
        let out = "An asterisk (*) denotes that a network service is disabled.\nWi-Fi\n*Disabled LAN\nThunderbolt Bridge\n\n";
        assert_eq!(
            parse_services(out),
            vec!["Wi-Fi".to_string(), "Thunderbolt Bridge".to_string()]
        );
    }

    #[test]
    fn parses_enabled_flag() {
        assert!(parse_proxy_enabled(
            "Enabled: Yes\nServer: 127.0.0.1\nPort: 7897\n"
        ));
        assert!(!parse_proxy_enabled("Enabled: No\nServer: \nPort: 0\n"));
    }

    #[test]
    fn parses_endpoint_from_getwebproxy_output() {
        let ep = parse_endpoint(
            "Enabled: Yes\nServer: 127.0.0.1\nPort: 7890\nAuthenticated Proxy Enabled: 0\n",
        )
        .unwrap();
        assert_eq!(ep.host, "127.0.0.1");
        assert_eq!(ep.port, 7890);
        assert!(parse_endpoint("Enabled: No\nServer: \nPort: 0\n").is_none());
    }

    #[derive(Clone)]
    struct FakeBackend {
        inner: Arc<FakeState>,
    }

    struct FakeState {
        services: Vec<String>,
        enabled: Mutex<HashSet<String>>,
        fail_on: Mutex<Vec<String>>,
        /// 各服务 enable 前的原设置（模拟第三方客户端已配置的场景）。
        presets: Mutex<BTreeMap<String, ServiceSnapshot>>,
        /// restore_service 调用记录（断言还原内容用）。
        restored: Mutex<BTreeMap<String, ServiceSnapshot>>,
    }

    impl FakeBackend {
        fn new(services: &[&str]) -> Self {
            Self {
                inner: Arc::new(FakeState {
                    services: services.iter().map(|s| s.to_string()).collect(),
                    enabled: Mutex::new(HashSet::new()),
                    fail_on: Mutex::new(vec![]),
                    presets: Mutex::new(BTreeMap::new()),
                    restored: Mutex::new(BTreeMap::new()),
                }),
            }
        }

        fn with_preset(self, service: &str, snap: ServiceSnapshot) -> Self {
            self.inner
                .presets
                .lock()
                .unwrap()
                .insert(service.to_string(), snap);
            self
        }
    }

    impl SysProxyBackend for FakeBackend {
        fn list_services(&self) -> Result<Vec<String>, String> {
            Ok(self.inner.services.clone())
        }
        fn enable_service(&self, service: &str, _port: u16) -> Result<(), String> {
            if self
                .inner
                .fail_on
                .lock()
                .unwrap()
                .iter()
                .any(|s| s == service)
            {
                return Err(format!("boom on {service}"));
            }
            self.inner
                .enabled
                .lock()
                .unwrap()
                .insert(service.to_string());
            Ok(())
        }
        fn disable_service(&self, service: &str) -> Result<(), String> {
            self.inner.enabled.lock().unwrap().remove(service);
            Ok(())
        }
        fn service_snapshot(&self, service: &str) -> Result<ServiceSnapshot, String> {
            Ok(self
                .inner
                .presets
                .lock()
                .unwrap()
                .get(service)
                .cloned()
                .unwrap_or_default())
        }
        fn restore_service(&self, service: &str, snap: &ServiceSnapshot) -> Result<(), String> {
            self.inner
                .restored
                .lock()
                .unwrap()
                .insert(service.to_string(), snap.clone());
            Ok(())
        }
    }

    type FakeManager = SysProxyManager<FakeBackend>;

    fn manager_with(backend: FakeBackend, dir: &tempfile::TempDir) -> FakeManager {
        SysProxyManager::new(backend, dir.path().join("journal.json"))
    }

    #[test]
    fn enable_writes_journal_disable_clears() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = manager_with(FakeBackend::new(&["Wi-Fi", "Ethernet"]), &dir);
        mgr.enable(7897).unwrap();
        assert!(mgr.status().enabled);
        assert_eq!(mgr.status().port, 7897);
        assert!(dir.path().join("journal.json").exists());

        mgr.disable();
        assert!(!mgr.status().enabled);
        assert!(!dir.path().join("journal.json").exists());
    }

    #[test]
    fn enable_all_failed_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let b = FakeBackend::new(&["Wi-Fi"]);
        b.inner.fail_on.lock().unwrap().push("Wi-Fi".into());
        let mgr = manager_with(b, &dir);
        assert!(mgr.enable(7897).is_err());
        assert!(!mgr.status().enabled);
    }

    #[test]
    fn stale_journal_triggers_restore() {
        let dir = tempfile::tempdir().unwrap();
        // 第一次会话：开启后「崩溃」（manager 直接丢弃，journal 残留）。
        {
            let mgr = manager_with(FakeBackend::new(&["Wi-Fi"]), &dir);
            let _ = mgr.enable(7897);
        }
        assert!(dir.path().join("journal.json").exists());

        // 第二次会话启动：识别残留并还原。
        let mgr = manager_with(FakeBackend::new(&["Wi-Fi"]), &dir);
        assert!(mgr.recover_stale());
        assert!(!dir.path().join("journal.json").exists());
    }

    /// 关闭时按快照还原第三方客户端的原设置（而不是一刀切关掉）。
    #[test]
    fn disable_restores_original_settings() {
        let dir = tempfile::tempdir().unwrap();
        let preset = ServiceSnapshot {
            web: Some(ProxyEndpoint {
                host: "127.0.0.1".into(),
                port: 7890,
            }),
            secure: None,
            socks: None,
        };
        let backend = FakeBackend::new(&["Wi-Fi"]).with_preset("Wi-Fi", preset.clone());
        let mgr = manager_with(backend.clone(), &dir);
        mgr.enable(7897).unwrap();
        mgr.disable();

        let restored = backend.inner.restored.lock().unwrap();
        let snap = restored.get("Wi-Fi").expect("restore called");
        assert_eq!(
            snap.web,
            Some(ProxyEndpoint {
                host: "127.0.0.1".into(),
                port: 7890,
            })
        );
        assert_eq!(snap.secure, None);
    }

    /// 无原设置（原本全关）时，关闭走逐服务还原路径且各槽位归零（等价全关）。
    #[test]
    fn disable_with_empty_preset_restores_off() {
        let dir = tempfile::tempdir().unwrap();
        let backend = FakeBackend::new(&["Wi-Fi"]);
        let mgr = manager_with(backend.clone(), &dir);
        mgr.enable(7897).unwrap();
        mgr.disable();
        let restored = backend.inner.restored.lock().unwrap();
        let snap = restored.get("Wi-Fi").expect("restore called");
        assert_eq!(*snap, ServiceSnapshot::default());
        assert!(!dir.path().join("journal.json").exists());
    }

    /// 真实切换一次系统代理并还原（改动本机 networksetup 设置，测试结束即还原）。
    /// 防护：若当前已有任何服务开启了代理（用户可能在用别的客户端），直接跳过。
    #[test]
    #[ignore = "touches real system proxy settings"]
    fn live_networksetup_toggle_and_restore() {
        let services = NetworkSetup.list_services().unwrap();
        assert!(!services.is_empty(), "no network services found");
        let already_on = services.iter().any(|svc| {
            NetworkSetup::run(&["-getwebproxy", svc])
                .map(|out| parse_proxy_enabled(&out))
                .unwrap_or(false)
        });
        if already_on {
            eprintln!("skip: system proxy already in use by another client");
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let mgr = SysProxyManager::new(NetworkSetup, dir.path().join("journal.json"));
        mgr.enable(17899).unwrap();
        assert!(mgr.status().enabled);
        let out = NetworkSetup::run(&["-getwebproxy", &services[0]]).unwrap();
        assert!(parse_proxy_enabled(&out), "proxy should be enabled");

        mgr.disable();
        let out = NetworkSetup::run(&["-getwebproxy", &services[0]]).unwrap();
        assert!(!parse_proxy_enabled(&out), "proxy should be restored");
    }
}
