//! #601 轻量退出后的内核守护（macOS 首发）：GUI 进程已退出、内核与系统
//! 代理保持运行期间，由用户级 LaunchAgent 周期探活；内核死亡或卡死时
//! **仅恢复系统代理**（走 journal 快照既有还原通道），然后退出。
//!
//! 边界（docs/reliability/lightweight-exit-guard.md）：
//! - 不自动重启内核（重启 = 用户重开应用，收养或冷启动走既有链路）；
//! - 恢复前检查系统代理归属：仍指向本应用端口才动手，第三方改写后不动；
//! - GUI 重新启动（app.pid 存活）即让位，收养/冷启动由 GUI 既有链路负责；
//! - 生命周期上限 24h，防陈旧代理驻留。
//!
//! 注册时机：轻量退出（`lightweight_process_exit`）；
//! GUI 启动时注销（bootout + 移除 plist），双保险防陈旧代理驻留。
//! TUN 模式轻量期内核死亡 = TUN 设备随进程消失、流量自动回退物理网络，
//! 无系统代理黑洞，守护仅覆盖系统代理模式（差异记录在案）。

use std::time::{Duration, Instant};

/// 工作模式参数：LaunchAgent 以本参数重入应用二进制。
pub const GUARD_ARG: &str = "--kernel-guard";
const LABEL: &str = "com.richard.crossbow.kernel-guard";
const WATCH_TICK: Duration = Duration::from_secs(10);
/// 生命周期上限：超过后守护自退，避免陈旧代理驻留。
const MAX_LIFETIME: Duration = Duration::from_secs(24 * 3600);

/// 守护决策矩阵（纯函数，便于测试）：
/// - GUI 存活 → 让位（收养/冷启动归 GUI 链路）；
/// - journal 不存在 → 无接管契约，让位；
/// - 生命周期到顶 → 让位（陈旧守护不驻留）；
/// - 代理端口仍在监听 → 继续观察（轻量态正常）；
/// - 端口无监听（内核死亡/卡死）→ 恢复系统代理。
fn decide(
    gui_alive: bool,
    journal_exists: bool,
    port_listening: bool,
    lifetime_over: bool,
) -> GuardAction {
    if gui_alive || !journal_exists || lifetime_over {
        return GuardAction::Yield;
    }
    if port_listening {
        GuardAction::Watch
    } else {
        GuardAction::Restore
    }
}

#[derive(Debug, PartialEq, Eq)]
enum GuardAction {
    /// GUI 回来了/无契约/生命周期到顶：守护退出
    Yield,
    /// 轻量态正常：继续探活
    Watch,
    /// 内核死亡/卡死且代理黑洞：仅恢复系统代理后退出
    Restore,
}

/// journal 里的混合端口（sysproxy 指向的端口）。
fn read_journal_port(journal: &std::path::Path) -> Option<u16> {
    let raw = std::fs::read_to_string(journal).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    value.get("port").and_then(|x| x.as_u64()).map(|p| p as u16)
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use crate::network::sysproxy::SysProxyManager;
    use std::path::{Path, PathBuf};

    /// 轻量退出时注册并立即启动守护。失败只留日志，不阻塞退出。
    pub(crate) fn register_and_start(data_dir: &Path) {
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(error) => {
                eprintln!("kernel-guard: 无法定位程序，跳过注册：{error}");
                return;
            }
        };
        let uid = unsafe { libc::getuid() };
        let plist_path = plist_path();
        if let Some(parent) = plist_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let log_dir = data_dir.join("logs");
        let _ = std::fs::create_dir_all(&log_dir);
        let plist = plist_body(
            &exe.to_string_lossy(),
            &log_dir.join("kernel-guard.log").to_string_lossy(),
        );
        if let Err(error) = std::fs::write(&plist_path, plist) {
            eprintln!("kernel-guard: plist 写入失败：{error}");
            return;
        }
        let domain = format!("gui/{uid}");
        // 已注册的旧实例先退场（幂等），再装载并立即启动一次
        let _ = std::process::Command::new("launchctl")
            .args(["bootout", &format!("{domain}/{LABEL}")])
            .status();
        if let Err(error) = std::process::Command::new("launchctl")
            .args(["bootstrap", &domain])
            .arg(&plist_path)
            .status()
        {
            eprintln!("kernel-guard: bootstrap 失败：{error}");
            return;
        }
        let _ = std::process::Command::new("launchctl")
            .args(["kickstart", &format!("{domain}/{LABEL}")])
            .status();
    }

    /// GUI 启动时注销守护：轻量会话已结束（收养或冷启动由 GUI 接管）。
    pub(crate) fn unregister() {
        let uid = unsafe { libc::getuid() };
        let plist_path = plist_path();
        let _ = std::process::Command::new("launchctl")
            .args(["bootout", &format!("gui/{uid}/{LABEL}")])
            .status();
        let _ = std::fs::remove_file(&plist_path);
    }

    /// 守护主循环（`--kernel-guard` 模式，不返回）。纯本地文件与系统
    /// 状态判定，无网络访问；决策矩阵见 `decide`。
    pub fn guard_main() -> ! {
        let data_dir = data_dir_path();
        let journal = data_dir.join("sysproxy-journal.json");
        let started = Instant::now();
        loop {
            let gui_alive = gui_alive(&data_dir);
            let journal_exists = journal.exists();
            let port = if journal_exists {
                read_journal_port(&journal)
            } else {
                None
            };
            let port_listening = port.is_some_and(crate::lifecycle::exit_flow::port_has_listener);
            let lifetime_over = started.elapsed() > MAX_LIFETIME;
            match decide(gui_alive, journal_exists, port_listening, lifetime_over) {
                GuardAction::Yield => std::process::exit(0),
                GuardAction::Watch => std::thread::sleep(WATCH_TICK),
                GuardAction::Restore => {
                    restore_and_notify(&data_dir, port);
                    std::process::exit(0);
                }
            }
        }
    }

    /// 恢复系统代理（journal 快照既有还原通道，内含归属检查）+ 系统通知。
    fn restore_and_notify(data_dir: &Path, port: Option<u16>) {
        let backend = crate::network::sysproxy::NetworkSetup;
        let manager = SysProxyManager::new(backend, data_dir.join("sysproxy-journal.json"));
        let restored = manager.recover_stale();
        eprintln!(
            "kernel-guard: 内核已死亡，系统代理{}（原端口 {:?}）",
            if restored {
                "已还原"
            } else {
                "还原失败"
            },
            port
        );
        notify_result(restored);
    }

    /// 守护进程直接投递系统通知（进程即将退出，托盘/界面不存在）。
    fn notify_result(restored: bool) {
        let mut notification = notify_rust::Notification::new();
        if restored {
            notification
                .summary("Crossbow 内核守护")
                .body("内核在轻量期退出，系统代理已还原为直连，网络未受影响。重新打开 Crossbow 可重新接管。");
        } else {
            notification
                .summary("Crossbow 内核守护")
                .body("内核在轻量期退出，系统代理还原失败——请在系统设置中手动关闭代理，或重新打开 Crossbow。");
        }
        let _ = notification.show();
    }

    /// GUI 存活判定：app.pid 存在且该进程仍在运行（轻量退出时 app.pid 已
    /// 被 release_session 移除；GUI 重启后重新写入）。
    fn gui_alive(data_dir: &Path) -> bool {
        let path = data_dir.join("app.pid");
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return false;
        };
        raw.trim()
            .parse::<u32>()
            .map(|pid| unsafe { libc::kill(pid as i32, 0) == 0 })
            .unwrap_or(false)
    }

    /// 守护运行的数据目录（LaunchAgent 环境下 HOME 由 launchd 提供）。
    fn data_dir_path() -> PathBuf {
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        home.join("Library/Application Support/com.richard.crossbow")
    }

    fn plist_path() -> PathBuf {
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        home.join(format!("Library/LaunchAgents/{LABEL}.plist"))
    }

    fn plist_body(exe: &str, log_path: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
    <string>{GUARD_ARG}</string>
  </array>
  <key>StandardOutPath</key>
  <string>{log_path}</string>
  <key>StandardErrorPath</key>
  <string>{log_path}</string>
</dict>
</plist>
"#
        )
    }
}

#[cfg(target_os = "macos")]
pub(crate) use imp::{guard_main, register_and_start, unregister};

#[cfg(test)]
mod tests {
    use super::GuardAction;
    use super::{decide, read_journal_port};

    #[test]
    fn decision_matrix() {
        // 轻量态正常：观察
        assert_eq!(decide(false, true, true, false), GuardAction::Watch);
        // 内核死亡/卡死（端口无监听）：恢复系统代理
        assert_eq!(decide(false, true, false, false), GuardAction::Restore);
        // GUI 重启：让位（即使内核死亡——GUI 链路自愈 + 冷启动接管）
        assert_eq!(decide(true, true, false, false), GuardAction::Yield);
        // journal 不存在：无接管契约（用户关闭了接管/TUN 模式）
        assert_eq!(decide(false, false, false, false), GuardAction::Yield);
        assert_eq!(decide(false, false, true, false), GuardAction::Yield);
        // 生命周期到顶：让位（陈旧守护不驻留）
        assert_eq!(decide(false, true, true, true), GuardAction::Yield);
    }

    #[test]
    fn read_journal_port_parses_mixed_port() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sysproxy-journal.json");
        std::fs::write(&path, r#"{"port": 7898, "enabled": true}"#).unwrap();
        assert_eq!(read_journal_port(&path), Some(7898));
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(read_journal_port(&path), None);
    }
}
