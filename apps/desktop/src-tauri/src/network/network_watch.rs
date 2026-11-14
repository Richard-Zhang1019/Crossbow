//! #490：macOS 主网络服务变化 / 睡眠唤醒后的分层恢复（观察 + 有限动作）。
//!
//! 机制对齐参考实现：Clash Verge Rev 的主服务监听 + 重应用代理，
//! Hiddify 的「先复查当前端点，失败才行动」。Crossbow 的差异处理：
//! - **事件驱动**：SCDynamicStore 订阅 `State:/Network/Global/IPv4` 与
//!   `State:/Network/Global/DNS`——主服务切换、接口/地址变化、睡眠唤醒
//!   （唤醒重建主服务）都会触发；系统自身写代理不变更这两把钥匙
//!   （代理设置在 Setup:/Service 段），不存在自我回环。不轮询。
//! - **去抖**：2 秒窗口合并事件风暴（切网/唤醒常伴随多轮 store 变更）。
//! - **等待新路径就绪**：直连基准 204 成功或 8 秒预算耗尽后才检查
//!   （登录门户 / 无网场景由此天然跳过动作）。
//! - **分层决策**（`decide` 纯函数）：内核/端口/控制器不健康 → 持续故障
//!   才进入有限重启（预算与冷却复用 #350 常量）；内核健康时按接管归属
//!   处理——Crossbow 拥有意图且主服务指向不一致才重应用系统代理；
//!   主服务被第三方/手动代理占据则绝不动写（#458 归属语义）。
//! - **TUN 不在本模块**：捕获路径漂移由 5s 健康看门狗的既有漂移检测与
//!   重建立流程处理（`takeover_drift` → `reestablish_tun_if_enabled`）。
//!   本模块的触发只服务系统代理层（该层没有周期性等价物）。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use core_foundation::string::CFString;
use system_configuration::dynamic_store::{
    SCDynamicStore, SCDynamicStoreBuilder, SCDynamicStoreCallBackContext,
};

use tauri::{Emitter, Manager};

use crate::AppState;

/// 事件去抖窗口：切网/唤醒常伴随多轮 store 变更，合并为一次检查。
pub const DEBOUNCE: Duration = Duration::from_secs(2);
/// 新路径就绪等待预算：直连 204 仍未成功则本轮不动作（门户/无网）。
const SETTLE_BUDGET: Duration = Duration::from_secs(8);
const SETTLE_INTERVAL: Duration = Duration::from_secs(1);
const DIRECT_PROBES: &[&str] = &[
    "http://connect.rom.miui.com/generate_204",
    "http://wifi.vivo.com.cn/generate_204",
];

/// 分层决策（纯函数，单测覆盖判定矩阵）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// 内核与当前出口健康：保持现有连接，不写任何设置。
    Keep,
    /// Crossbow 仍拥有接管意图、内核健康，但主服务没有指向我们的代理：
    /// 重应用系统代理（主服务切换/唤醒后 macOS 按服务保存配置的缺口）。
    Reapply,
    /// 主服务被第三方或手动代理占据：绝不改写（#458 归属语义）。
    SkipThirdParty,
    /// 直连基准失败：登录门户 / 无网，本轮不动作。
    SkipNoConnectivity,
    /// 内核持续故障（端口/控制器层）：进入既有有限重启 + 冷却。
    WedgeRestart,
}

/// 纯决策：输入一轮 #488 分层证据的关键位与主服务观测到的代理指向。
/// `primary_observed = None` 表示主服务当前没有任何代理配置。
pub fn decide(
    mixed_port_ok: bool,
    controller_ok: bool,
    direct_cn_ok: bool,
    primary_observed: Option<(&str, &str, u16)>,
    our_port: u16,
) -> Action {
    if !direct_cn_ok {
        // 门户 / 无网：等下一次事件，绝不在此状态写代理或重启。
        return Action::SkipNoConnectivity;
    }
    if !mixed_port_ok || !controller_ok {
        // 端口/控制器层故障且物理链路正常 = 内核自身持续故障。
        return Action::WedgeRestart;
    }
    // 内核健康。接管归属一致性独立于出口探测结果：主服务丢了代理时，
    // 经混合端口的显式探测依然成功，不能据此跳过重应用。
    match primary_observed {
        Some((_, host, port)) if host == "127.0.0.1" && port == our_port => Action::Keep,
        Some(_) => Action::SkipThirdParty,
        None => Action::Reapply,
    }
}

static WEDGE_COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static WEDGE_COOLDOWN_UNTIL_MS: AtomicU64 = AtomicU64::new(0);

pub fn spawn(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let (tx, rx) = mpsc::channel::<()>();
        fn on_change(
            _store: SCDynamicStore,
            _changed: core_foundation::array::CFArray<core_foundation::string::CFString>,
            tx: &mut mpsc::Sender<()>,
        ) {
            let _ = tx.send(());
        }
        let store = match SCDynamicStoreBuilder::new("CrossbowNetworkWatch")
            .callback_context(SCDynamicStoreCallBackContext {
                callout: on_change,
                info: tx,
            })
            .build()
        {
            Some(store) => store,
            None => {
                eprintln!("network watch: could not open SCDynamicStore");
                return;
            }
        };
        let keys = core_foundation::array::CFArray::from_CFTypes(&[
            CFString::new("State:/Network/Global/IPv4"),
            CFString::new("State:/Network/Global/DNS"),
        ]);
        let patterns: core_foundation::array::CFArray<CFString> =
            core_foundation::array::CFArray::from_CFTypes(&[]);
        if !store.set_notification_keys(&keys, &patterns) {
            eprintln!("network watch: could not subscribe to store keys");
            return;
        }
        let source = match store.create_run_loop_source() {
            Some(source) => source,
            None => {
                eprintln!("network watch: could not create run loop source");
                return;
            }
        };
        // SAFETY: kCFRunLoopDefaultMode 是 CoreFoundation 的常量静态。
        core_foundation::runloop::CFRunLoop::get_current().add_source(&source, unsafe {
            core_foundation::runloop::kCFRunLoopDefaultMode
        });
        eprintln!("network watch: armed; re-applies system proxy on primary service changes");

        // 事件处理线程：runloop 回调只投递（不可阻塞）。首个事件开启
        // 去抖窗口，窗口内事件合并为一次检查；处理串行执行。
        std::thread::spawn(move || loop {
            if rx.recv().is_err() {
                return;
            }
            std::thread::sleep(DEBOUNCE);
            while rx.try_recv().is_ok() {
                // 吸收去抖窗口内的后续事件
            }
            handle_network_change(&app);
        });

        core_foundation::runloop::CFRunLoop::run_current();
    });
}

fn handle_network_change(handle: &tauri::AppHandle) {
    let state = handle.state::<AppState>();
    if state.exit.ensure_open().is_err() {
        return;
    }
    let tun_on = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .engine
        .tun_enable;
    if tun_on {
        // TUN 捕获路径漂移交给既有 5s 看门狗的漂移检测与重建立流程。
        return;
    }
    if !state.sysproxy.status().enabled {
        // Crossbow 没有接管系统代理：无论谁写入了什么，都不动（#458）。
        return;
    }
    if !matches!(
        state.core().status(),
        crate::engine::manager::CoreStatus::Running
    ) {
        // 内核停止/崩溃由既有 sysproxy 兜底（#356）处理。
        return;
    }

    // 等新路径具备基本连接条件（登录门户/无网时预算耗尽 → 本轮不动作）。
    let direct = direct_probe_ok_until(SETTLE_BUDGET);

    // 一轮 #488 分层取证（分类 + 系统代理实际指向）。
    let Some(evidence) = crate::diagnostics::fault_evidence::run_round(&state) else {
        return; // 代已变化（切换配置/关闭接管/换核）：本轮作废
    };

    let port = evidence.mixed_port;
    // 主服务名（取证里是 "服务名 → host:port" 文本；这里按主服务对齐）。
    let Some(primary_service) = primary_service() else {
        // 默认路由设备不在 networksetup 映射里（utun 等 = 其他 VPN 接管）：
        // 主服务身份未知，不动作也不猜测归属。
        eprintln!("network watch: default route is not a known network service; skipped");
        return;
    };
    let primary_observed = evidence.sysproxy_observed.iter().find_map(|entry| {
        let (service, rest) = entry.split_once(" → ")?;
        if service != primary_service {
            return None;
        }
        let (host, port_text) = rest.rsplit_once(':')?;
        Some((service, host, port_text.parse::<u16>().ok()?))
    });

    let action = decide(
        evidence.mixed_port_ok,
        evidence.controller.is_ok(),
        direct,
        primary_observed,
        port,
    );
    match action {
        Action::Keep => {
            // 健康出口不替换；若 #488 有未恢复记录，此时确认恢复
            // （恢复 = 首个成功的经代理请求，验收第 2 条的时间记录）。
            if let Some(existing) = crate::diagnostics::fault_evidence::load(&state) {
                if existing.resolved_at_ms.is_none() {
                    crate::diagnostics::fault_evidence::confirm_recovery(&state, &existing);
                }
            }
        }
        Action::SkipThirdParty => {
            eprintln!("network watch: primary service proxy is owned by another client; skipped");
        }
        Action::SkipNoConnectivity => {
            eprintln!("network watch: new path not connected yet; skipped");
        }
        Action::WedgeRestart => {
            // 持续故障才有限重启：预算/冷却复用 #350 常量。
            let cooldown_left = WEDGE_COOLDOWN_UNTIL_MS
                .load(Ordering::SeqCst)
                .saturating_sub(now_ms());
            if cooldown_left > 0 {
                return;
            }
            let count = WEDGE_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
            if count < u32::from(crate::engine::timing::WEDGE_RESTART_BUDGET) {
                eprintln!("network watch: core wedge evidence ({count})");
                return;
            }
            WEDGE_COUNT.store(0, Ordering::SeqCst);
            WEDGE_COOLDOWN_UNTIL_MS.store(
                now_ms() + crate::engine::timing::WEDGE_COOLDOWN.as_millis() as u64,
                Ordering::SeqCst,
            );
            eprintln!("network watch: restarting wedged core (sysproxy mode)");
            if let Err(error) = restart_wedged_core(&state, handle) {
                eprintln!("network watch: wedge restart failed: {error}");
            }
        }
        Action::Reapply => {
            // 执行前意图复核：接管可能已在等待期间被用户关闭。
            if !state.sysproxy.status().enabled {
                return;
            }
            match state.sysproxy.enable(port) {
                Ok(()) => {
                    let _ = handle.emit("sysproxy://status", state.sysproxy.status());
                    crate::refresh_tray(handle);
                    eprintln!("network watch: system proxy re-applied to the new primary service");
                }
                Err(error) => {
                    eprintln!("network watch: re-apply failed: {error}");
                }
            }
        }
    }
    // 出口层证据：经代理 HTTPS 不健康时落盘（开记录抑制，#488 语义）。
    if !evidence.proxy_https.is_ok() {
        crate::diagnostics::fault_evidence::record_round_if_needed(&state, evidence);
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 主接口 → 主网络服务名。默认路由设备（utun 等）不在 networksetup 的
/// 服务映射里时返回 None——主服务身份未知（其他 VPN 接管了默认路由），
/// 调用侧必须跳过动作，不能猜测归属。
fn primary_device() -> Option<String> {
    let output = std::process::Command::new("route")
        .args(["-n", "get", "default"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().find_map(|line| {
        let (key, value) = line.trim().split_once(':')?;
        (key.trim() == "interface").then(|| value.trim().to_string())
    })
}

/// networksetup -listnetworkserviceorder 的 (设备 → 服务) 映射。
fn device_service_map() -> Vec<(String, String)> {
    let Ok(output) = std::process::Command::new("networksetup")
        .args(["-listnetworkserviceorder"])
        .output()
    else {
        return vec![];
    };
    if !output.status.success() {
        return vec![];
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .filter_map(|line| {
            let inner = line.trim().strip_prefix('(')?.strip_suffix(')')?;
            let (port_part, device_part) = inner.split_once(", Device: ")?;
            let service = port_part
                .strip_prefix("Hardware Port: ")?
                .trim()
                .to_string();
            Some((device_part.trim().to_string(), service))
        })
        .collect()
}

fn primary_service() -> Option<String> {
    let device = primary_device()?;
    device_service_map()
        .into_iter()
        .find(|(known, _)| known == &device)
        .map(|(_, service)| service)
}

fn direct_probe_ok_until(budget: Duration) -> bool {
    let agent = ureq::AgentBuilder::new()
        .try_proxy_from_env(false)
        .timeout(Duration::from_secs(2))
        .build();
    let started = Instant::now();
    loop {
        for url in DIRECT_PROBES {
            match agent.head(url).call() {
                Ok(_) | Err(ureq::Error::Status(_, _)) => return true,
                Err(_) => continue,
            }
        }
        if started.elapsed() >= budget {
            return false;
        }
        std::thread::sleep(SETTLE_INTERVAL);
    }
}

/// 系统代理模式下的内核楔死重启：与 core_restart 命令同一套锁与启动链。
fn restart_wedged_core(state: &AppState, handle: &tauri::AppHandle) -> Result<(), String> {
    let _restart = state
        .core_restart_op
        .try_lock()
        .map_err(|_| "内核正在重启".to_string())?;
    crate::notifications::publish(
        handle,
        crate::notifications::Notice::new(
            crate::notifications::Kind::Progress,
            "core.restarting",
            "core-restart",
        ),
    );
    let _operation = state.core_op.lock().unwrap_or_else(|e| e.into_inner());
    state.exit.ensure_open()?;
    if !state.core().stop() {
        return Err("旧内核未退出，已中止重启".into());
    }
    let result = crate::start_core_locked(state).and_then(|()| crate::wait_core_running(state));
    let code = if result.is_ok() {
        "core.restarted"
    } else {
        "core.restartFailed"
    };
    crate::notifications::result(handle, "core-restart", code, result.clone());
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const PORT: u16 = 7898;

    fn base(
        port_ok: bool,
        controller_ok: bool,
        direct_ok: bool,
        primary: Option<(&str, &str, u16)>,
    ) -> Action {
        decide(port_ok, controller_ok, direct_ok, primary, PORT)
    }

    /// 直连失败（门户/无网）：任何状态都不动作，避免误接管或误重启。
    #[test]
    fn no_connectivity_always_skips() {
        assert_eq!(
            base(false, false, false, Some(("Wi-Fi", "127.0.0.1", 9000))),
            Action::SkipNoConnectivity
        );
        assert_eq!(base(true, true, false, None), Action::SkipNoConnectivity);
    }

    /// 内核层故障且物理链路正常 = 持续楔死，进入有限重启分支。
    #[test]
    fn kernel_layer_failure_triggers_wedge_restart() {
        assert_eq!(base(false, true, true, None), Action::WedgeRestart);
        assert_eq!(
            base(true, false, true, Some(("Wi-Fi", "127.0.0.1", PORT))),
            Action::WedgeRestart
        );
    }

    /// 主服务仍指向 Crossbow：保持现有连接（健康出口不被替换）。
    #[test]
    fn consistent_primary_is_kept_even_when_proxy_https_fails() {
        assert_eq!(
            base(true, true, true, Some(("Wi-Fi", "127.0.0.1", PORT))),
            Action::Keep
        );
    }

    /// 主服务被其他客户端或手动代理占据：绝不改写（#458 归属语义）。
    #[test]
    fn third_party_primary_is_never_overwritten() {
        assert_eq!(
            base(true, true, true, Some(("Wi-Fi", "127.0.0.1", 7890))),
            Action::SkipThirdParty
        );
        assert_eq!(
            base(true, true, true, Some(("Wi-Fi", "192.168.1.1", 8080))),
            Action::SkipThirdParty
        );
    }

    /// 主服务没有任何代理（切网/唤醒后的典型缺口）：重应用系统代理。
    #[test]
    fn missing_primary_proxy_is_reapplied() {
        assert_eq!(base(true, true, true, None), Action::Reapply);
    }
}
