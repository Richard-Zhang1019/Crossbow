//! 系统代理管理：按平台后端设置/还原系统级代理，带崩溃兜底 journal。
//!
//! 兜底思路（DESIGN §4.4 状态一致性守护）：开启成功后把状态写入 journal
//! 文件；正常关闭/退出时清除。若上次会话异常退出（journal 残留），本次启动
//! 先执行一次全量还原——保证「任何路径退出都不留脏系统代理」。

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
    /// 为单个服务还原（关闭三类代理，不动其他配置）。
    fn disable_service(&self, service: &str) -> Result<(), String>;
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

#[derive(Debug, Serialize, Deserialize)]
struct Journal {
    #[allow(dead_code)]
    enabled: bool,
    port: u16,
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

    /// 对所有网络服务开启代理。逐服务尽力执行，全部失败才报错。
    pub fn enable(&self, port: u16) -> Result<(), String> {
        let services = self.backend.list_services()?;
        if services.is_empty() {
            return Err("no network services found".into());
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
        self.write_journal(port);
        Ok(())
    }

    /// 还原所有服务的代理设置（幂等：未开启时也安全执行，用于退出兜底）。
    pub fn disable(&self) {
        if let Ok(services) = self.backend.list_services() {
            for svc in &services {
                let _ = self.backend.disable_service(svc);
            }
        }
        *self.state.lock().unwrap() = None;
        let _ = std::fs::remove_file(&self.journal_path);
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

    /// 启动时兜底：上次会话残留的开启状态 → 全量还原。返回是否发生了还原。
    pub fn recover_stale(&self) -> bool {
        let Ok(raw) = std::fs::read_to_string(&self.journal_path) else {
            return false;
        };
        let stale = serde_json::from_str::<Journal>(&raw)
            .ok()
            .is_some_and(|j| j.enabled);
        if stale {
            self.disable();
        } else {
            let _ = std::fs::remove_file(&self.journal_path);
        }
        stale
    }

    fn write_journal(&self, port: u16) {
        let j = Journal {
            enabled: true,
            port,
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

    struct FakeBackend {
        services: Vec<String>,
        enabled: Mutex<HashSet<String>>,
        fail_on: Vec<String>,
    }

    impl FakeBackend {
        fn new(services: &[&str]) -> Self {
            Self {
                services: services.iter().map(|s| s.to_string()).collect(),
                enabled: Mutex::new(HashSet::new()),
                fail_on: vec![],
            }
        }
    }

    impl SysProxyBackend for FakeBackend {
        fn list_services(&self) -> Result<Vec<String>, String> {
            Ok(self.services.clone())
        }
        fn enable_service(&self, service: &str, _port: u16) -> Result<(), String> {
            if self.fail_on.iter().any(|s| s == service) {
                return Err(format!("boom on {service}"));
            }
            self.enabled.lock().unwrap().insert(service.to_string());
            Ok(())
        }
        fn disable_service(&self, service: &str) -> Result<(), String> {
            self.enabled.lock().unwrap().remove(service);
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
        let mut b = FakeBackend::new(&["Wi-Fi"]);
        b.fail_on = vec!["Wi-Fi".into()];
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
