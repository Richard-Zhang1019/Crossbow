//! 系统代理管理：按平台后端设置/还原系统级代理，带崩溃兜底 journal。
//!
//! 兜底思路（DESIGN §4.4 状态一致性守护）：开启成功后把状态写入 journal
//! 文件；正常关闭/退出时清除。若上次会话异常退出（journal 残留），本次启动
//! 先执行一次全量还原——保证「任何路径退出都不留脏系统代理」。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

use serde::{Deserialize, Serialize};

/// 默认绕过列表（本机/局域网直连）。
#[cfg(target_os = "macos")]
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

    /// 一次开启全部服务；平台后端可合并管理员授权。
    fn enable_services(&self, services: &[String], port: u16) -> Result<(), String> {
        for service in services {
            self.enable_service(service, port)
                .map_err(|error| format!("{service}: {error}"))?;
        }
        Ok(())
    }

    /// 尽力关闭全部服务，汇总错误以保留恢复记录。
    fn disable_services(&self, services: &[String]) -> Result<(), String> {
        let errors: Vec<_> = services
            .iter()
            .filter_map(|service| {
                self.disable_service(service)
                    .err()
                    .map(|error| format!("{service}: {error}"))
            })
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("；"))
        }
    }

    /// 尽力恢复全部快照，单个服务失败不阻止其他服务恢复。
    fn restore_services(
        &self,
        snapshots: &BTreeMap<String, ServiceSnapshot>,
    ) -> Result<(), String> {
        let errors: Vec<_> = snapshots
            .iter()
            .filter_map(|(service, snapshot)| {
                self.restore_service(service, snapshot)
                    .err()
                    .map(|error| format!("{service}: {error}"))
            })
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("；"))
        }
    }
}

/// macOS `networksetup` 实现。
#[cfg(target_os = "macos")]
pub struct NetworkSetup;

#[cfg(target_os = "macos")]
impl NetworkSetup {
    /// 只读命令无需提权，固定系统路径以避免 PATH 劫持。
    fn run(args: &[&str]) -> Result<String, String> {
        let out = std::process::Command::new("/usr/sbin/networksetup")
            .args(args)
            .output()
            .map_err(|e| format!("run networksetup: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "networksetup {}: {}",
                args.join(" "),
                Self::error_detail(&out)
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    /// 保留两条输出流与退出码，系统工具可能将错误写入 stdout。
    fn error_detail(out: &std::process::Output) -> String {
        format!(
            "{}\n{}\n退出状态：{}",
            String::from_utf8_lossy(&out.stdout).trim(),
            String::from_utf8_lossy(&out.stderr).trim(),
            out.status
        )
        .trim()
        .to_string()
    }

    /// 将每个参数作为独立 shell 字面量，网卡名称和快照内容不可成为命令。
    fn command(args: &[&str]) -> String {
        std::iter::once("/usr/sbin/networksetup")
            .chain(args.iter().copied())
            .map(|arg| format!("'{}'", arg.replace('\'', "'\\''")))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// 原始 Enabled 位与端点都一致才可跳过恢复，避免漏掉已开启但端口无效的槽位。
    fn slot_matches(output: &str, expected: &Option<ProxyEndpoint>) -> bool {
        parse_proxy_enabled(output) == expected.is_some() && parse_endpoint(output) == *expected
    }

    /// 按三类代理的真实开关位判断是否需要关闭。
    fn service_enabled(service: &str) -> Result<bool, String> {
        let mut enabled = false;
        for getter in [
            "-getwebproxy",
            "-getsecurewebproxy",
            "-getsocksfirewallproxy",
        ] {
            enabled |= parse_proxy_enabled(&Self::run(&[getter, service])?);
        }
        Ok(enabled)
    }

    /// 逐槽位和绕过列表核对快照，未提交的授权无需额外回滚授权。
    fn snapshot_matches(service: &str, snapshot: &ServiceSnapshot) -> Result<bool, String> {
        let mut matches = true;
        for (getter, expected) in [
            ("-getwebproxy", &snapshot.web),
            ("-getsecurewebproxy", &snapshot.secure),
            ("-getsocksfirewallproxy", &snapshot.socks),
        ] {
            matches &= Self::slot_matches(&Self::run(&[getter, service])?, expected);
        }
        if let Some(bypass) = &snapshot.bypass {
            matches &= parse_bypass_domains(&Self::run(&["-getproxybypassdomains", service])?)
                .join("\n")
                == *bypass;
        }
        Ok(matches)
    }

    /// 一次原生授权执行整批写入；恢复时尽力处理所有命令。
    fn batch_script(commands: &[String], restoring: bool) -> String {
        let mut script = String::from("exec 1>&2\n");
        if restoring {
            script.push_str("status=0\n");
            for command in commands {
                script.push_str(&format!("{command} || status=$?\n"));
            }
            script.push_str("exit \"$status\"");
        } else {
            script.push_str("set -e\n");
            script.push_str(&commands.join("\n"));
        }
        script
    }

    /// 密码仅交由 macOS 授权窗口处理，不持久化授权或安装 root 常驻进程。
    fn run_batch(commands: &[String], restoring: bool) -> Result<(), String> {
        if commands.is_empty() {
            return Ok(());
        }
        let script = Self::batch_script(commands, restoring);
        let out = std::process::Command::new("/usr/bin/osascript")
            .args(["-e", "on run argv\ndo shell script (item 1 of argv) with administrator privileges\nend run", "--", &script])
            .output().map_err(|error| format!("无法打开系统管理员授权：{error}"))?;
        if out.status.success() {
            return Ok(());
        }
        let detail = Self::error_detail(&out);
        if detail.contains("(-128)") {
            Err("已取消系统代理管理员授权，代理设置未提交".into())
        } else {
            Err(format!("系统代理管理员操作失败：{detail}"))
        }
    }

    /// 生成单个服务的开启命令，供整批执行与单服务调用共用。
    fn enable_commands(service: &str, port: u16) -> Vec<String> {
        let port = port.to_string();
        let mut commands: Vec<_> = [
            "-setwebproxy",
            "-setsecurewebproxy",
            "-setsocksfirewallproxy",
        ]
        .iter()
        .map(|setter| Self::command(&[setter, service, "127.0.0.1", &port]))
        .collect();
        let mut bypass = vec!["-setproxybypassdomains", service];
        bypass.extend(BYPASS_DOMAINS.iter().copied());
        commands.push(Self::command(&bypass));
        commands
    }

    /// 生成三个代理槽位的关闭命令。
    fn disable_commands(service: &str) -> Vec<String> {
        [
            "-setwebproxystate",
            "-setsecurewebproxystate",
            "-setsocksfirewallproxystate",
        ]
        .iter()
        .map(|setter| Self::command(&[setter, service, "off"]))
        .collect()
    }

    /// 按快照生成恢复命令，保留原代理端点与绕过列表。
    fn restore_commands(service: &str, snapshot: &ServiceSnapshot) -> Vec<String> {
        let mut commands = Vec::new();
        for (setter, state_setter, endpoint) in [
            ("-setwebproxy", "-setwebproxystate", &snapshot.web),
            (
                "-setsecurewebproxy",
                "-setsecurewebproxystate",
                &snapshot.secure,
            ),
            (
                "-setsocksfirewallproxy",
                "-setsocksfirewallproxystate",
                &snapshot.socks,
            ),
        ] {
            commands.push(match endpoint {
                Some(endpoint) => {
                    Self::command(&[setter, service, &endpoint.host, &endpoint.port.to_string()])
                }
                None => Self::command(&[state_setter, service, "off"]),
            });
        }
        if let Some(bypass) = &snapshot.bypass {
            let mut args = vec!["-setproxybypassdomains", service];
            args.extend(bypass.lines());
            if args.len() == 2 {
                args.push("Empty");
            }
            commands.push(Self::command(&args));
        }
        commands
    }
}

#[cfg(target_os = "macos")]
impl SysProxyBackend for NetworkSetup {
    fn list_services(&self) -> Result<Vec<String>, String> {
        let out = Self::run(&["-listallnetworkservices"])?;
        Ok(parse_services(&out))
    }

    fn enable_service(&self, service: &str, port: u16) -> Result<(), String> {
        self.enable_services(&[service.to_string()], port)
    }

    fn disable_service(&self, service: &str) -> Result<(), String> {
        self.disable_services(&[service.to_string()])
    }

    fn enable_services(&self, services: &[String], port: u16) -> Result<(), String> {
        let commands: Vec<_> = services
            .iter()
            .flat_map(|service| Self::enable_commands(service, port))
            .collect();
        Self::run_batch(&commands, false)
    }

    fn disable_services(&self, services: &[String]) -> Result<(), String> {
        let mut commands = Vec::new();
        let mut errors = Vec::new();
        for service in services {
            let enabled = match Self::service_enabled(service) {
                Ok(enabled) => enabled,
                Err(error) => {
                    errors.push(format!("{service}: {error}"));
                    continue;
                }
            };
            if enabled {
                commands.extend(Self::disable_commands(service));
            }
        }
        if let Err(error) = Self::run_batch(&commands, true) {
            errors.push(error);
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("；"))
        }
    }

    fn restore_services(
        &self,
        snapshots: &BTreeMap<String, ServiceSnapshot>,
    ) -> Result<(), String> {
        let mut commands = Vec::new();
        let mut errors = Vec::new();
        for (service, snapshot) in snapshots {
            // 取消授权没有写入；相同状态直接跳过，回滚不再重复弹授权窗口。
            let matches = match Self::snapshot_matches(service, snapshot) {
                Ok(matches) => matches,
                Err(error) => {
                    errors.push(format!("{service}: {error}"));
                    continue;
                }
            };
            if !matches {
                commands.extend(Self::restore_commands(service, snapshot));
            }
        }
        if let Err(error) = Self::run_batch(&commands, true) {
            errors.push(error);
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("；"))
        }
    }

    fn service_snapshot(&self, service: &str) -> Result<ServiceSnapshot, String> {
        Ok(ServiceSnapshot {
            windows: None,
            web: parse_endpoint(&Self::run(&["-getwebproxy", service])?),
            secure: parse_endpoint(&Self::run(&["-getsecurewebproxy", service])?),
            socks: parse_endpoint(&Self::run(&["-getsocksfirewallproxy", service])?),
            bypass: Some(
                parse_bypass_domains(&Self::run(&["-getproxybypassdomains", service])?).join("\n"),
            ),
        })
    }

    fn restore_service(&self, service: &str, snap: &ServiceSnapshot) -> Result<(), String> {
        self.restore_services(&BTreeMap::from([(service.to_string(), snap.clone())]))
    }
}

#[cfg(windows)]
#[path = "sysproxy_windows.rs"]
mod windows;
#[cfg(windows)]
pub use windows::WinInet;

/// 非 macOS 且非 Windows 平台桩：保持编译通过。
#[cfg(not(any(target_os = "macos", windows)))]
pub struct UnsupportedBackend;

#[cfg(not(any(target_os = "macos", windows)))]
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
#[cfg(windows)]
pub(crate) type PlatformBackend = WinInet;
#[cfg(not(any(target_os = "macos", windows)))]
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

/// Preserve Windows per-connection settings verbatim, including PAC/WPAD.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowsProxyConfig {
    pub flags: u32,
    pub server: String,
    pub bypass: String,
    pub pac_url: String,
}

#[cfg(any(windows, test))]
impl WindowsProxyConfig {
    pub fn endpoint(&self, protocol: &str) -> Option<ProxyEndpoint> {
        if self.flags & 2 == 0 {
            return None;
        } // PROXY_TYPE_PROXY
        let raw = if self.server.contains('=') {
            self.server
                .split(';')
                .filter_map(|entry| entry.trim().split_once('='))
                .find(|(name, _)| name.trim().eq_ignore_ascii_case(protocol))?
                .1
        } else {
            &self.server
        };
        let (host, port) = raw.trim().rsplit_once(':')?;
        Some(ProxyEndpoint {
            host: host.trim().trim_matches(['[', ']']).to_string(),
            port: port.parse().ok()?,
        })
    }
}

/// 一个网络服务的代理设置快照；None = 该槽位原本关闭。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows: Option<WindowsProxyConfig>,
    pub web: Option<ProxyEndpoint>,
    pub secure: Option<ProxyEndpoint>,
    pub socks: Option<ProxyEndpoint>,
    /// Windows：ProxyOverride 原文；macOS：逐行保存的绕过域名。
    #[serde(default)]
    pub bypass: Option<String>,
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
    // 只串行化代理操作，不持锁调用内核或主线程，避免退出时跨线程死锁。
    operation: Mutex<()>,
    shutting_down: AtomicBool,
    /// Some(port) = 已开启。
    state: Mutex<Option<u16>>,
    journal_path: PathBuf,
}

/// The exact system proxy state before TUN took ownership of routing. Persisted
/// until TUN is disabled so a restart or failed startup can restore it.
#[derive(Serialize, Deserialize)]
pub(crate) struct SuspendedSysProxy {
    snapshot: BTreeMap<String, ServiceSnapshot>,
    journal: Option<Vec<u8>>,
    enabled_port: Option<u16>,
}

impl<B: SysProxyBackend> SysProxyManager<B> {
    /// 退出开始后拒绝新开启请求；已有授权完成后先回滚，退出清理才可继续。
    pub(crate) fn begin_shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
    }

    /// 读取退出状态，不在主事件循环上等待 core_op。
    fn ensure_active(&self) -> Result<(), String> {
        if self.shutting_down.load(Ordering::SeqCst) {
            Err("应用正在退出，未开启系统代理".into())
        } else {
            Ok(())
        }
    }

    fn tun_journal_path(&self) -> PathBuf {
        self.journal_path.with_file_name("tun-proxy-journal.json")
    }

    pub fn new(backend: B, journal_path: PathBuf) -> Self {
        Self {
            backend,
            operation: Mutex::new(()),
            shutting_down: AtomicBool::new(false),
            state: Mutex::new(None),
            journal_path,
        }
    }

    /// 对所有网络服务开启代理。先保存完整快照与恢复日志；任一服务设置
    /// 失败时回滚全部服务，避免留下 HTTP/HTTPS/SOCKS 半配置状态。
    pub fn enable(&self, port: u16) -> Result<(), String> {
        let _operation = self
            .operation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.ensure_active()?;
        self.enable_locked(port)
    }

    /// 已持有代理操作锁的内部实现。
    fn enable_locked(&self, port: u16) -> Result<(), String> {
        // A previous attempt may have changed some services before its
        // rollback failed. Recover that snapshot before taking a new one;
        // otherwise the original settings would be overwritten permanently.
        if self.journal_path.exists() {
            self.disable_locked()?;
        }
        let services = self.backend.list_services()?;
        if services.is_empty() {
            return Err("no network services found".into());
        }
        let mut snapshot = BTreeMap::new();
        for svc in &services {
            snapshot.insert(svc.clone(), self.backend.service_snapshot(svc)?);
        }
        // 写盘必须发生在任何 networksetup 修改之前，异常退出也能恢复。
        self.write_journal(port, &snapshot)?;
        if let Err(error) = self
            .backend
            .enable_services(&services, port)
            .and_then(|()| self.ensure_active())
        {
            let mut rollback_errors: Vec<_> = self
                .backend
                .restore_services(&snapshot)
                .err()
                .into_iter()
                .collect();
            if rollback_errors.is_empty() {
                if let Err(remove_error) = std::fs::remove_file(&self.journal_path) {
                    rollback_errors.push(format!("清理恢复日志失败: {remove_error}"));
                }
            }
            let detail = if rollback_errors.is_empty() {
                String::new()
            } else {
                format!("；回滚未完成：{}", rollback_errors.join("；"))
            };
            return Err(format!("{error}{detail}"));
        }
        *self.state.lock().unwrap() = Some(port);
        Ok(())
    }

    /// When TUN suspended another client's proxy, transfer that original
    /// snapshot to Crossbow's proxy journal without briefly re-enabling it.
    /// The two Crossbow switches can then be controlled independently.
    pub fn enable_during_tun(&self, port: u16) -> Result<(), String> {
        let _operation = self
            .operation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.ensure_active()?;
        self.enable_during_tun_locked(port)
    }

    /// 已持有代理操作锁的内部实现。
    fn enable_during_tun_locked(&self, port: u16) -> Result<(), String> {
        let tun_journal = self.tun_journal_path();
        let saved = match std::fs::read(&tun_journal) {
            Ok(saved) => saved,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return self.enable_locked(port);
            }
            Err(error) => return Err(format!("读取 TUN 代理恢复日志失败：{error}")),
        };
        if self.journal_path.exists() {
            return Err("系统代理恢复日志已存在，无法接管 TUN 暂停的代理".into());
        }
        let suspended: SuspendedSysProxy = serde_json::from_slice(&saved)
            .map_err(|error| format!("read TUN proxy journal: {error}"))?;
        let original = if suspended.enabled_port.is_some() {
            let raw = suspended
                .journal
                .as_deref()
                .ok_or("TUN 代理恢复日志缺少原始系统代理快照")?;
            serde_json::from_slice::<Journal>(raw)
                .map_err(|error| format!("解析原始系统代理快照失败：{error}"))?
                .snapshot
        } else {
            suspended.snapshot
        };
        let services = self.backend.list_services()?;
        if services.is_empty() {
            return Err("no network services found".into());
        }
        let mut current = BTreeMap::new();
        for service in &services {
            current.insert(service.clone(), self.backend.service_snapshot(service)?);
        }
        self.write_journal(port, &original)?;
        let result = self
            .backend
            .enable_services(&services, port)
            .and_then(|()| self.ensure_active())
            .and_then(|()| {
                std::fs::remove_file(&tun_journal)
                    .map_err(|error| format!("清理 TUN 代理恢复日志失败：{error}"))
            });
        if let Err(error) = result {
            let rollback_errors: Vec<_> = self
                .backend
                .restore_services(&current)
                .err()
                .into_iter()
                .collect();
            if rollback_errors.is_empty() {
                let _ = std::fs::remove_file(&self.journal_path);
            }
            let rollback = if rollback_errors.is_empty() {
                String::new()
            } else {
                format!("；回滚未完成：{}", rollback_errors.join("；"))
            };
            return Err(format!("开启系统代理失败：{error}{rollback}"));
        }
        *self.state.lock().unwrap() = Some(port);
        Ok(())
    }

    /// While TUN remains on, disabling Crossbow's proxy must leave macOS
    /// proxy settings off. Defer the pre-Crossbow snapshot until TUN exits.
    pub fn disable_during_tun(&self) -> Result<(), String> {
        let _operation = self
            .operation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.disable_during_tun_locked()
    }

    /// 已持有代理操作锁的内部实现。
    fn disable_during_tun_locked(&self) -> Result<(), String> {
        if self.state.lock().unwrap().is_none() {
            return Ok(());
        }
        let journal = self
            .read_journal()
            .ok_or("系统代理恢复日志无法读取，未覆盖现有代理设置")?;
        let tun_journal = self.tun_journal_path();
        let previous_tun_journal = match std::fs::read(&tun_journal) {
            Ok(saved) => Some(saved),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!("读取 TUN 代理恢复日志失败：{error}")),
        };
        let services = self.backend.list_services()?;
        let mut current = BTreeMap::new();
        for service in &services {
            current.insert(service.clone(), self.backend.service_snapshot(service)?);
        }
        let suspended = SuspendedSysProxy {
            snapshot: journal.snapshot,
            journal: None,
            enabled_port: None,
        };
        if previous_tun_journal.is_none() {
            std::fs::write(
                &tun_journal,
                serde_json::to_vec(&suspended).map_err(|error| error.to_string())?,
            )
            .map_err(|error| format!("保存 TUN 代理恢复日志失败：{error}"))?;
        }
        let result = self.backend.disable_services(&services).and_then(|()| {
            std::fs::remove_file(&self.journal_path)
                .map_err(|error| format!("清理系统代理恢复日志失败：{error}"))
        });
        if let Err(error) = result {
            let rollback_errors: Vec<_> = self
                .backend
                .restore_services(&current)
                .err()
                .into_iter()
                .collect();
            if previous_tun_journal.is_none() && rollback_errors.is_empty() {
                let _ = std::fs::remove_file(&tun_journal);
            }
            let rollback = if rollback_errors.is_empty() {
                String::new()
            } else {
                format!("；回滚未完成：{}", rollback_errors.join("；"))
            };
            return Err(format!("关闭系统代理失败：{error}{rollback}"));
        }
        *self.state.lock().unwrap() = None;
        Ok(())
    }

    /// 还原代理设置：优先按 journal 里的原设置快照逐服务恢复；无快照（老格式
    /// 或快照失败）退回全量关闭。幂等，用于退出兜底。
    pub fn disable(&self) -> Result<(), String> {
        let _operation = self
            .operation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.disable_locked()
    }

    /// 已持有代理操作锁的内部实现。
    fn disable_locked(&self) -> Result<(), String> {
        let journal = self.read_journal();
        if self.journal_path.exists() && journal.is_none() {
            return Err("系统代理恢复日志无法读取，未覆盖现有代理设置".into());
        }
        if journal.is_none() && self.state.lock().unwrap().is_none() {
            // Crossbow does not own the current proxy. In particular, exiting
            // while TUN is active must not clear another client's settings.
            return Ok(());
        }
        let result = if let Some(snapshot) = journal.map(|j| j.snapshot).filter(|s| !s.is_empty()) {
            self.backend.restore_services(&snapshot)
        } else {
            self.backend
                .disable_services(&self.backend.list_services()?)
        };
        // 失败时保留恢复记录和内存状态，后续可重试。
        result.map_err(|error| format!("还原系统代理失败：{error}"))?;
        if let Err(error) = std::fs::remove_file(&self.journal_path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(format!("清理系统代理恢复日志失败：{error}"));
            }
        }
        *self.state.lock().unwrap() = None;
        Ok(())
    }

    /// TUN must leave every network service with its system proxy disabled.
    /// `disable` restores the pre-Crossbow snapshot, which may point to an old
    /// local proxy with no listener and keep browsers offline even while TUN
    /// itself is healthy.
    pub fn suspend_for_tun(&self) -> Result<SuspendedSysProxy, String> {
        let _operation = self
            .operation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.suspend_for_tun_locked()
    }

    /// TUN 健康检查只清理外部代理；等待授权锁之后重新核对所有权。
    pub(crate) fn clear_external_for_tun(&self) -> Result<(), String> {
        let _operation = self
            .operation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if self.shutting_down.load(Ordering::SeqCst)
            || self.state.lock().unwrap().is_some()
            || self.external_endpoints().is_empty()
        {
            return Ok(());
        }
        self.suspend_for_tun_locked().map(|_| ())
    }

    /// 已持有代理操作锁的内部实现。
    fn suspend_for_tun_locked(&self) -> Result<SuspendedSysProxy, String> {
        let tun_journal = self.tun_journal_path();
        if let Ok(saved) = std::fs::read(&tun_journal) {
            let suspended = serde_json::from_slice(&saved)
                .map_err(|error| format!("read TUN proxy journal: {error}"))?;
            // Another proxy client may re-enable its system proxy while TUN is
            // running. Keep the original snapshot, but clear the live settings.
            self.backend
                .disable_services(&self.backend.list_services()?)?;
            return Ok(suspended);
        }
        let services = self.backend.list_services()?;
        if services.is_empty() {
            return Err("no network services found".into());
        }
        let mut snapshot = BTreeMap::new();
        for service in &services {
            snapshot.insert(service.clone(), self.backend.service_snapshot(service)?);
        }
        let suspended = SuspendedSysProxy {
            snapshot,
            journal: std::fs::read(&self.journal_path).ok(),
            enabled_port: *self.state.lock().unwrap(),
        };
        let journal = serde_json::to_vec(&suspended)
            .map_err(|error| format!("serialize TUN proxy journal: {error}"))?;
        std::fs::write(&tun_journal, journal)
            .map_err(|error| format!("save TUN proxy journal: {error}"))?;
        if let Err(error) = self.backend.disable_services(&services) {
            let rollback = self.backend.restore_services(&suspended.snapshot);
            if rollback.is_ok() {
                let _ = std::fs::remove_file(&tun_journal);
            }
            let detail = rollback
                .err()
                .map(|error| format!("；回滚未完成：{error}"))
                .unwrap_or_default();
            return Err(format!("暂停系统代理失败：{error}{detail}"));
        }
        *self.state.lock().unwrap() = None;
        let _ = std::fs::remove_file(&self.journal_path);
        Ok(suspended)
    }

    /// Restore the exact pre-TUN settings after either a failed start or a
    /// successful exit from TUN. Keep the journal if restoration fails.
    pub fn restore_after_failed_tun(&self, suspended: SuspendedSysProxy) -> Result<(), String> {
        let _operation = self
            .operation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.restore_after_failed_tun_locked(suspended)
    }

    /// 已持有代理操作锁的内部实现。
    fn restore_after_failed_tun_locked(&self, suspended: SuspendedSysProxy) -> Result<(), String> {
        self.backend.restore_services(&suspended.snapshot)?;
        match suspended.journal {
            Some(journal) => std::fs::write(&self.journal_path, journal)
                .map_err(|error| format!("restore system proxy journal: {error}"))?,
            None => {
                let _ = std::fs::remove_file(&self.journal_path);
            }
        }
        *self.state.lock().unwrap() = suspended.enabled_port;
        std::fs::remove_file(self.tun_journal_path())
            .map_err(|error| format!("remove TUN proxy journal: {error}"))?;
        Ok(())
    }

    pub fn restore_after_tun(&self) -> Result<(), String> {
        let _operation = self
            .operation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.restore_after_tun_locked()
    }

    /// 已持有代理操作锁的内部实现。
    fn restore_after_tun_locked(&self) -> Result<(), String> {
        if self.state.lock().unwrap().is_some() {
            return Ok(());
        }
        let path = self.tun_journal_path();
        let Ok(saved) = std::fs::read(path) else {
            return Ok(());
        };
        let suspended = serde_json::from_slice(&saved)
            .map_err(|error| format!("read TUN proxy journal: {error}"))?;
        self.restore_after_failed_tun_locked(suspended)
    }

    /// A cold launch has no Crossbow listener yet. If Crossbow owned the
    /// pre-TUN proxy, restore the settings that preceded Crossbow instead of
    /// pointing the system at its now-dead mixed port.
    pub fn recover_tun_without_core(&self) -> Result<(), String> {
        let _operation = self
            .operation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.recover_tun_without_core_locked()
    }

    /// 已持有代理操作锁的内部实现。
    fn recover_tun_without_core_locked(&self) -> Result<(), String> {
        let path = self.tun_journal_path();
        let Ok(saved) = std::fs::read(path) else {
            return Ok(());
        };
        let mut suspended: SuspendedSysProxy = serde_json::from_slice(&saved)
            .map_err(|error| format!("read TUN proxy journal: {error}"))?;
        if suspended.enabled_port.is_some() {
            suspended.snapshot = suspended
                .journal
                .as_deref()
                .and_then(|raw| serde_json::from_slice::<Journal>(raw).ok())
                .map(|journal| journal.snapshot)
                .unwrap_or_default();
            suspended.journal = None;
            suspended.enabled_port = None;
        }
        self.restore_after_failed_tun_locked(suspended)
    }

    /// 各服务当前生效的 HTTP/HTTPS/SOCKS 代理端点（含第三方设置的）。
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
                    if let Some(ep) = snap.socks {
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
        if !self.journal_path.exists() {
            return false;
        }
        if let Err(error) = self.disable() {
            eprintln!("sysproxy stale recovery failed: {error}");
            false
        } else {
            true
        }
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

    fn write_journal(
        &self,
        port: u16,
        snapshot: &BTreeMap<String, ServiceSnapshot>,
    ) -> Result<(), String> {
        let j = Journal {
            port,
            snapshot: snapshot.clone(),
        };
        if let Some(parent) = self.journal_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建代理恢复目录失败: {e}"))?;
        }
        std::fs::write(
            &self.journal_path,
            serde_json::to_vec(&j).map_err(|e| format!("序列化代理恢复日志失败: {e}"))?,
        )
        .map_err(|e| format!("写入代理恢复日志失败: {e}"))
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

/// `networksetup -getproxybypassdomains` 在没有条目时输出一句说明，
/// 有条目时每行一个域名。空列表仍需作为快照保存，恢复时传入 Empty。
#[cfg(target_os = "macos")]
fn parse_bypass_domains(output: &str) -> Vec<String> {
    let lines = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    lines
        .filter(|line| {
            !line.starts_with("There aren't any bypass domains")
                && !line.starts_with("There are no bypass domains")
                && !line.starts_with("Domains bypassed by the proxy server:")
        })
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
#[cfg(any(target_os = "macos", test))]
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
    fn windows_protocol_map_and_single_server_are_parsed_exactly() {
        let settings = WindowsProxyConfig {
            flags: 3,
            server: "http=127.0.0.1:7897;https=127.0.0.1:7898;socks=[::1]:7899".into(),
            ..WindowsProxyConfig::default()
        };
        assert_eq!(
            settings.endpoint("https"),
            Some(ProxyEndpoint {
                host: "127.0.0.1".into(),
                port: 7898
            })
        );
        assert_eq!(
            settings.endpoint("socks"),
            Some(ProxyEndpoint {
                host: "::1".into(),
                port: 7899
            })
        );
        let single = WindowsProxyConfig {
            server: "localhost:7897".into(),
            ..settings.clone()
        };
        assert_eq!(single.endpoint("https").unwrap().port, 7897);
        let off = WindowsProxyConfig {
            flags: 1,
            ..settings
        };
        assert_eq!(off.endpoint("https"), None);
    }

    #[test]
    fn windows_pac_and_bypass_survive_proxy_journal_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let original = ServiceSnapshot {
            windows: Some(WindowsProxyConfig {
                flags: 13,
                server: "http=proxy.example:8080;https=secure.example:8443".into(),
                bypass: "<local>;*.example;host with spaces".into(),
                pac_url: "https://config.example/proxy.pac".into(),
            }),
            ..ServiceSnapshot::default()
        };
        let backend = FakeBackend::new(&["Windows"]).with_preset("Windows", original.clone());
        let manager = manager_with(backend.clone(), &dir);
        manager.enable(7897).unwrap();
        manager.disable().unwrap();
        assert_eq!(
            backend.inner.restored.lock().unwrap().get("Windows"),
            Some(&original)
        );
        let legacy: ServiceSnapshot =
            serde_json::from_str(r#"{"web":null,"secure":null,"socks":null,"bypass":"<local>"}"#)
                .unwrap();
        assert!(legacy.windows.is_none());
    }

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
        restore_fail_on: Mutex<Vec<String>>,
        before_enable: Mutex<Option<std::sync::Arc<dyn Fn() + Send + Sync>>>,
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
                    restore_fail_on: Mutex::new(vec![]),
                    before_enable: Mutex::new(None),
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
            let before_enable = self.inner.before_enable.lock().unwrap().clone();
            if let Some(before_enable) = before_enable {
                before_enable();
            }
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
            if self
                .inner
                .restore_fail_on
                .lock()
                .unwrap()
                .iter()
                .any(|name| name == service)
            {
                return Err(format!("restore failed on {service}"));
            }
            self.inner.enabled.lock().unwrap().remove(service);
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

        mgr.disable().unwrap();
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

    /// 部分网卡开启失败时，已修改与未修改的服务都应恢复原快照。
    #[test]
    fn partial_enable_failure_restores_all_services() {
        let dir = tempfile::tempdir().unwrap();
        let backend = FakeBackend::new(&["Ethernet", "Wi-Fi"]);
        backend.inner.fail_on.lock().unwrap().push("Wi-Fi".into());
        let mgr = manager_with(backend.clone(), &dir);
        assert!(mgr.enable(7897).is_err());
        assert!(!mgr.status().enabled);
        assert!(backend.inner.enabled.lock().unwrap().is_empty());
        assert_eq!(backend.inner.restored.lock().unwrap().len(), 2);
        assert!(!dir.path().join("journal.json").exists());
    }

    /// 恢复失败须保留日志并继续其他服务；修复权限后同一实例可重试。
    #[test]
    fn failed_rollback_keeps_journal_and_retry_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let backend = FakeBackend::new(&["Ethernet", "Wi-Fi"]);
        backend.inner.fail_on.lock().unwrap().push("Wi-Fi".into());
        backend
            .inner
            .restore_fail_on
            .lock()
            .unwrap()
            .push("Ethernet".into());
        let mgr = manager_with(backend.clone(), &dir);
        assert!(mgr.enable(7897).unwrap_err().contains("回滚未完成"));
        assert!(dir.path().join("journal.json").exists());
        assert_eq!(backend.inner.restored.lock().unwrap().len(), 2);
        backend.inner.fail_on.lock().unwrap().clear();
        backend.inner.restore_fail_on.lock().unwrap().clear();
        mgr.enable(7897).unwrap();
        assert!(mgr.status().enabled);
        mgr.disable().unwrap();
        assert!(!mgr.status().enabled);
        assert!(!dir.path().join("journal.json").exists());
    }

    /// 退出清理必须等待尚未完成的授权写入，不能提前删除恢复日志。
    #[test]
    fn cleanup_waits_for_pending_enable_before_clearing_journal() {
        use std::sync::mpsc;
        use std::time::Duration;
        let dir = tempfile::tempdir().unwrap();
        let backend = FakeBackend::new(&["Wi-Fi"]);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        *backend.inner.before_enable.lock().unwrap() = Some(Arc::new(move || {
            entered_tx.send(()).unwrap();
            release_rx.lock().unwrap().recv().unwrap();
        }));
        let manager = Arc::new(manager_with(backend.clone(), &dir));
        let enabling = manager.clone();
        let enable = std::thread::spawn(move || enabling.enable(7897));
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let (cleanup_tx, cleanup_rx) = mpsc::channel();
        let cleaning = manager.clone();
        let cleanup = std::thread::spawn(move || {
            let result = cleaning.disable();
            cleanup_tx.send(()).unwrap();
            result
        });
        let completed_early = cleanup_rx.recv_timeout(Duration::from_millis(100)).is_ok();
        let journal_preserved = dir.path().join("journal.json").exists();
        release_tx.send(()).unwrap();
        enable.join().unwrap().unwrap();
        cleanup.join().unwrap().unwrap();
        assert!(!completed_early, "清理不能越过等待中的授权");
        assert!(journal_preserved, "授权结束前必须保留恢复快照");
        assert!(backend.inner.enabled.lock().unwrap().is_empty());
        assert!(!manager.status().enabled);
        assert!(!dir.path().join("journal.json").exists());
    }

    /// 已进入退出流程的实例不能再次开启代理。
    #[test]
    fn shutdown_rejects_new_enable_requests() {
        let dir = tempfile::tempdir().unwrap();
        let backend = FakeBackend::new(&["Wi-Fi"]);
        let manager = manager_with(backend.clone(), &dir);
        manager.begin_shutdown();
        assert!(manager.enable(7897).unwrap_err().contains("正在退出"));
        assert!(backend.inner.enabled.lock().unwrap().is_empty());
        assert!(!dir.path().join("journal.json").exists());
    }

    /// 授权等待期间开始退出，稍后成功的写入须回滚而不能报告已开启。
    #[test]
    fn shutdown_during_authorization_rolls_back_late_enable() {
        use std::sync::mpsc;
        use std::time::Duration;
        let dir = tempfile::tempdir().unwrap();
        let backend = FakeBackend::new(&["Wi-Fi"]);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        *backend.inner.before_enable.lock().unwrap() = Some(Arc::new(move || {
            entered_tx.send(()).unwrap();
            release_rx.lock().unwrap().recv().unwrap();
        }));
        let manager = Arc::new(manager_with(backend.clone(), &dir));
        let enabling = manager.clone();
        let enable = std::thread::spawn(move || enabling.enable(7897));
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        manager.begin_shutdown();
        release_tx.send(()).unwrap();
        assert!(enable.join().unwrap().unwrap_err().contains("正在退出"));
        assert!(backend.inner.enabled.lock().unwrap().is_empty());
        assert!(!manager.status().enabled);
        assert!(!dir.path().join("journal.json").exists());
    }

    /// 健康检查排队期间用户已开启代理时，不得沿旧状态将其关闭。
    #[test]
    fn queued_tun_health_cleanup_preserves_newly_enabled_proxy() {
        use std::sync::mpsc;
        use std::time::Duration;
        let dir = tempfile::tempdir().unwrap();
        let backend = FakeBackend::new(&["Wi-Fi"]).with_preset(
            "Wi-Fi",
            ServiceSnapshot {
                web: Some(ProxyEndpoint {
                    host: "127.0.0.1".into(),
                    port: 7890,
                }),
                ..ServiceSnapshot::default()
            },
        );
        let manager = Arc::new(manager_with(backend.clone(), &dir));
        manager.suspend_for_tun().unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        *backend.inner.before_enable.lock().unwrap() = Some(Arc::new(move || {
            entered_tx.send(()).unwrap();
            release_rx.lock().unwrap().recv().unwrap();
        }));
        let enabling = manager.clone();
        let enable = std::thread::spawn(move || enabling.enable_during_tun(7897));
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let cleaning = manager.clone();
        let cleanup = std::thread::spawn(move || cleaning.clear_external_for_tun());
        release_tx.send(()).unwrap();
        enable.join().unwrap().unwrap();
        cleanup.join().unwrap().unwrap();
        assert!(manager.status().enabled);
        assert!(backend.inner.enabled.lock().unwrap().contains("Wi-Fi"));
        assert!(dir.path().join("journal.json").exists());
        assert!(!manager.tun_journal_path().exists());
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
            windows: None,
            bypass: None,
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
        mgr.disable().unwrap();

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
        mgr.disable().unwrap();
        let restored = backend.inner.restored.lock().unwrap();
        let snap = restored.get("Wi-Fi").expect("restore called");
        assert_eq!(*snap, ServiceSnapshot::default());
        assert!(!dir.path().join("journal.json").exists());
    }

    /// macOS 专属测试（networksetup 后端）——测试模块需在 Windows 上
    /// 编译（统一通知的 CI 回归步骤跨平台跑），此处的 NetworkSetup
    /// 引用必须随平台门控，否则 Windows 编译失败。
    #[cfg(target_os = "macos")]
    mod macos_networksetup {
        use super::*;

        /// 开启位为 Yes 的无效端点不能被误认为已经关闭。
        #[test]
        fn enabled_invalid_endpoint_does_not_match_disabled_slot() {
            let invalid = "Enabled: Yes\nServer: \nPort: 0\n";
            assert!(parse_proxy_enabled(invalid));
            assert!(!NetworkSetup::slot_matches(invalid, &None));
            assert!(NetworkSetup::slot_matches(
                "Enabled: No\nServer: \nPort: 0\n",
                &None
            ));
        }

        /// 真实系统工具把部分错误写在 stdout，通知必须保留该原因。
        #[test]
        fn networksetup_stdout_error_is_preserved() {
            let error = NetworkSetup::run(&["-getwebproxy", "__crossbow_nonexistent_service__"])
                .expect_err("虚构服务应失败");
            assert!(error.contains("** Error:"), "{error}");
        }

        /// 引号、换行与 shell 元字符必须原样传递，不能形成额外命令。
        #[test]
        fn networksetup_arguments_cannot_inject_shell_commands() {
            let arguments = [
                "Wi-Fi",
                "USB LAN's \"bridge\"\n$(printf injected); `printf injected`",
                "*.local",
            ];
            let command = NetworkSetup::command(&arguments).replacen(
                "'/usr/sbin/networksetup'",
                "/usr/bin/printf '%s\\0'",
                1,
            );
            let out = std::process::Command::new("/bin/sh")
                .args(["-c", &command])
                .output()
                .unwrap();
            assert!(out.status.success());
            let expected = arguments
                .iter()
                .flat_map(|value| value.as_bytes().iter().copied().chain([0]))
                .collect::<Vec<_>>();
            assert_eq!(out.stdout, expected);
        }

        /// 开启失败立即停止，恢复失败继续；两条路径都保留原退出码和原因。
        #[test]
        fn batch_stops_enable_but_continues_recovery_after_failure() {
            let commands = vec![
                "/bin/sh -c 'printf \"Command requires admin privileges\"; exit 14'".into(),
                "printf 'next-service'".into(),
            ];
            for restoring in [false, true] {
                let script = NetworkSetup::batch_script(&commands, restoring);
                let out = std::process::Command::new("/bin/sh")
                    .args(["-c", &script])
                    .output()
                    .unwrap();
                assert_eq!(out.status.code(), Some(14));
                let error = NetworkSetup::error_detail(&out);
                assert!(
                    error.contains("Command requires admin privileges"),
                    "{error}"
                );
                assert_eq!(error.contains("next-service"), restoring, "{error}");
            }
        }

        /// 相同状态的回滚不应再次请求管理员权限或修改系统。
        #[test]
        fn restoring_unchanged_state_needs_no_authorization() {
            let services = NetworkSetup.list_services().unwrap();
            let snapshots = services
                .iter()
                .map(|service| {
                    (
                        service.clone(),
                        NetworkSetup.service_snapshot(service).unwrap(),
                    )
                })
                .collect();
            NetworkSetup.restore_services(&snapshots).unwrap();
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

            mgr.disable().unwrap();
            let out = NetworkSetup::run(&["-getwebproxy", &services[0]]).unwrap();
            assert!(!parse_proxy_enabled(&out), "proxy should be restored");
        }
    }
}
