//! #490：事件驱动的 macOS 系统代理恢复。TUN 继续由现有健康监视器维护。
//! 纯决策可跨平台测试；平台订阅、权限与内核动作仅在 macOS 编译。

use super::sysproxy::RecoveryProxyState;
use std::time::{Duration, Instant};

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Keep,
    Reapply,
    Skip,
    CheckKernel,
}

fn decide(direct_ready: bool, ownership: RecoveryProxyState, kernel_ready: bool) -> Action {
    // 其他客户端/PAC 拥有主服务时，连内核重启也不执行。
    if !direct_ready || ownership == RecoveryProxyState::Foreign {
        Action::Skip
    } else if !kernel_ready {
        Action::CheckKernel
    } else if ownership == RecoveryProxyState::Missing {
        Action::Reapply
    } else {
        Action::Keep
    }
}

#[derive(Default)]
struct RestartBudget {
    used: u8,
    last: Option<Instant>,
}
impl RestartBudget {
    fn take(&mut self, now: Instant, limit: u8, cooldown: Duration) -> bool {
        if self.used >= limit
            || self
                .last
                .is_some_and(|last| now.duration_since(last) < cooldown)
        {
            return false;
        }
        self.used += 1;
        self.last = Some(now);
        true
    }
}

/// 服务名来自序号行，不是 Hardware Port（用户可重命名网络服务）。
fn parse_service_order(text: &str) -> Vec<(String, String)> {
    let mut service = None;
    let mut result = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix('(') {
            if let Some((number, name)) = rest.split_once(") ") {
                service = number.parse::<usize>().ok().map(|_| name.to_string());
                continue;
            }
        }
        if let Some((_, device)) = line.split_once(", Device: ") {
            if let (Some(name), Some(device)) = (service.take(), device.strip_suffix(')')) {
                // utun/隧道/不可识别接口由其拥有者维护，不猜测主物理网络。
                if device.starts_with("en") && device[2..].parse::<u32>().is_ok() {
                    result.push((device.into(), name));
                }
            }
        }
    }
    result
}

#[cfg(target_os = "macos")]
mod runtime {
    use super::*;
    use crate::diagnostics::fault_evidence::{self, Generation};
    use crate::lifecycle::authorization::spawn_background;
    use crate::AppState;
    use core_foundation::{array::CFArray, string::CFString};
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    };
    use system_configuration::dynamic_store::{
        SCDynamicStore, SCDynamicStoreBuilder, SCDynamicStoreCallBackContext,
    };
    use tauri::{Emitter, Manager};

    static EVENTS: AtomicU64 = AtomicU64::new(0);
    const DEBOUNCE: Duration = Duration::from_secs(2);
    const SETTLE: Duration = Duration::from_secs(8);

    fn trigger(tx: &SyncSender<()>) {
        EVENTS.fetch_add(1, Ordering::SeqCst);
        let _ = tx.try_send(()); // 有界、非阻塞；最新代号不丢失。
    }

    pub fn spawn(app: tauri::AppHandle) {
        let (tx, rx) = mpsc::sync_channel(1);
        // NSWorkspace 通知独立于地址/DNS 变化；同一 Wi-Fi 唤醒也会触发。
        let wake_tx = tx.clone();
        let _ = app.run_on_main_thread(move || {
            use objc2_app_kit::{NSWorkspace, NSWorkspaceDidWakeNotification};
            let center = NSWorkspace::sharedWorkspace().notificationCenter();
            let block = block2::RcBlock::new(
                move |_: std::ptr::NonNull<objc2_foundation::NSNotification>| trigger(&wake_tx),
            );
            // SAFETY: block 仅捕获 Send + Sync 的 channel，queue/object 均为空。
            let observer = unsafe {
                center.addObserverForName_object_queue_usingBlock(
                    Some(NSWorkspaceDidWakeNotification),
                    None,
                    None,
                    &block,
                )
            };
            // 应用级订阅与进程同寿命；center 持有 block，保留 token 直到进程退出。
            std::mem::forget(observer);
        });
        spawn_background(move || {
            fn on_change(_: SCDynamicStore, _: CFArray<CFString>, tx: &mut SyncSender<()>) {
                trigger(tx);
            }
            let Some(store) = SCDynamicStoreBuilder::new("CrossbowNetworkWatch")
                .callback_context(SCDynamicStoreCallBackContext {
                    callout: on_change,
                    info: tx.clone(),
                })
                .build()
            else {
                return;
            };
            let keys = CFArray::from_CFTypes(&[
                CFString::new("State:/Network/Global/IPv4"),
                CFString::new("State:/Network/Global/IPv6"),
                CFString::new("State:/Network/Global/DNS"),
            ]);
            if !store.set_notification_keys(&keys, &CFArray::<CFString>::from_CFTypes(&[])) {
                return;
            }
            let Some(source) = store.create_run_loop_source() else {
                return;
            };
            // SAFETY: CoreFoundation 的静态模式常量。
            core_foundation::runloop::CFRunLoop::get_current().add_source(&source, unsafe {
                core_foundation::runloop::kCFRunLoopDefaultMode
            });
            spawn_background(move || {
                let mut budget = RestartBudget::default();
                let mut blocked = None;
                while rx.recv().is_ok() {
                    // 固定窗口避免事件风暴令检查饥饿；后续事件使旧动作失效。
                    std::thread::sleep(DEBOUNCE);
                    while rx.try_recv().is_ok() {}
                    let event = EVENTS.load(Ordering::SeqCst);
                    // 单个事件最多三轮，处理暂时离线/观察器占用；不产生常驻轮询。
                    for _ in 0..3 {
                        if !handle_change(&app, event, &mut budget, &mut blocked, &tx)
                            || EVENTS.load(Ordering::SeqCst) != event
                        {
                            break;
                        }
                        std::thread::sleep(DEBOUNCE);
                    }
                }
            });
            core_foundation::runloop::CFRunLoop::run_current();
        });
    }

    fn primary_service() -> Option<String> {
        let output = std::process::Command::new("/sbin/route")
            .args(["-n", "get", "default"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let device = text.lines().find_map(|line| {
            let (key, value) = line.trim().split_once(':')?;
            (key.trim() == "interface").then(|| value.trim().to_string())
        })?;
        let order = std::process::Command::new("/usr/sbin/networksetup")
            .arg("-listnetworkserviceorder")
            .output()
            .ok()?;
        if !order.status.success() {
            return None;
        }
        parse_service_order(&String::from_utf8_lossy(&order.stdout))
            .into_iter()
            .find(|(known, _)| *known == device)
            .map(|(_, service)| service)
    }

    fn current(state: &AppState, gen: &Generation, event: u64, service: &str) -> bool {
        EVENTS.load(Ordering::SeqCst) == event
            && fault_evidence::recovery_current(state, gen)
            && primary_service().as_deref() == Some(service)
    }

    /// 端口/控制器至少两次失败；第二次发生在 core_op 内、停止进程之前。
    fn kernel_ready(state: &AppState, port: u16) -> bool {
        let port_ok = std::net::TcpStream::connect_timeout(
            &([127, 0, 0, 1], port).into(),
            Duration::from_secs(1),
        )
        .is_ok();
        let controller_ok = state.core().controller().is_some_and(|(port, secret)| {
            ureq::AgentBuilder::new()
                .try_proxy_from_env(false)
                .redirects(0)
                .timeout(Duration::from_secs(2))
                .build()
                .get(&format!("http://127.0.0.1:{port}/version"))
                .set("Authorization", &format!("Bearer {secret}"))
                .call()
                .is_ok_and(|resp| resp.status() == 200)
        });
        port_ok && controller_ok
    }

    fn handle_change(
        app: &tauri::AppHandle,
        event: u64,
        budget: &mut RestartBudget,
        blocked: &mut Option<(Generation, String)>,
        tx: &SyncSender<()>,
    ) -> bool {
        let state = app.state::<AppState>();
        let Some(gen) = fault_evidence::recovery_snapshot(&state) else {
            return false;
        };
        let Some(service) = primary_service() else {
            return false;
        };
        if blocked.as_ref() == Some(&(gen.clone(), service.clone())) {
            return false;
        }
        let deadline = Instant::now() + SETTLE;
        loop {
            if !current(&state, &gen, event, &service) {
                return false;
            }
            if fault_evidence::direct_ready(deadline) {
                break;
            }
            if Instant::now() >= deadline {
                return true;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let Some(evidence) = fault_evidence::inspect_network_change(app, &state, &gen) else {
            return true;
        };
        let Ok(owner) = state.sysproxy.recovery_state(&service, evidence.mixed_port) else {
            return false;
        };
        let action = decide(
            true,
            owner,
            evidence.mixed_port_ok && evidence.controller.is_ok(),
        );
        if action == Action::Skip || action == Action::Keep {
            return false;
        }
        if action == Action::CheckKernel {
            std::thread::sleep(DEBOUNCE);
        }
        // 排队期间的关开、配置切换、内核替换及新网络事件均使本轮作废。
        let Ok(_restart) = state.core_restart_op.try_lock() else {
            return true;
        };
        let Ok(_core) = state.core_op.try_lock() else {
            return true;
        };
        if !current(&state, &gen, event, &service) {
            return false;
        }
        let Ok(owner) = state.sysproxy.recovery_state(&service, evidence.mixed_port) else {
            return false;
        };
        if owner == RecoveryProxyState::Foreign {
            return false;
        }
        match action {
            Action::Reapply => {
                if !kernel_ready(&state, evidence.mixed_port)
                    || !current(&state, &gen, event, &service)
                {
                    return false;
                }
                let result = state
                    .sysproxy
                    .reapply_primary(&service, evidence.mixed_port);
                if let Err(error) = result {
                    *blocked = Some((gen, service)); // 自己的写事件不能形成失败重试循环。
                    crate::notifications::result(
                        app,
                        "network-proxy-recovery",
                        "tray.proxy",
                        Err(error),
                    );
                } else {
                    let _ = app.emit("sysproxy://status", state.sysproxy.status());
                    crate::refresh_tray(app);
                    let _ = tx.try_send(()); // 下一轮真正 HTTPS 成功后才记录恢复。
                }
            }
            Action::CheckKernel => {
                if kernel_ready(&state, evidence.mixed_port)
                    || !current(&state, &gen, event, &service)
                {
                    return false;
                }
                if !budget.take(
                    Instant::now(),
                    crate::engine::timing::WEDGE_RESTART_BUDGET,
                    crate::engine::timing::WEDGE_COOLDOWN,
                ) {
                    return false;
                }
                // 最多每会话两次，冷却不重置预算。仅重建已确认失败的同一配置。
                crate::notifications::publish(
                    app,
                    crate::notifications::Notice::new(
                        crate::notifications::Kind::Progress,
                        "core.restarting",
                        "network-core-restart",
                    ),
                );
                let result = if !state.core().stop() {
                    Err("旧内核未退出，已中止重启".into())
                } else {
                    crate::start_core_locked(&state).and_then(|()| crate::wait_core_running(&state))
                };
                let code = if result.is_ok() {
                    "core.restarted"
                } else {
                    "core.restartFailed"
                };
                crate::notifications::result(app, "network-core-restart", code, result);
                let _ = tx.try_send(());
            }
            Action::Keep | Action::Skip => {}
        }
        false
    }
}
#[cfg(target_os = "macos")]
pub use runtime::spawn;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn offline_or_foreign_owner_never_restarts_or_writes() {
        for healthy in [false, true] {
            for owner in [
                RecoveryProxyState::Owned,
                RecoveryProxyState::Missing,
                RecoveryProxyState::Foreign,
            ] {
                assert_eq!(decide(false, owner, healthy), Action::Skip);
            }
            assert_eq!(
                decide(true, RecoveryProxyState::Foreign, healthy),
                Action::Skip
            );
        }
        assert_eq!(decide(true, RecoveryProxyState::Owned, true), Action::Keep);
        assert_eq!(
            decide(true, RecoveryProxyState::Missing, true),
            Action::Reapply
        );
        assert_eq!(
            decide(true, RecoveryProxyState::Missing, false),
            Action::CheckKernel
        );
    }
    #[test]
    fn cooldown_does_not_replenish_restart_budget() {
        let mut budget = RestartBudget::default();
        let now = Instant::now();
        let cooldown = Duration::from_secs(90);
        assert!(budget.take(now, 2, cooldown));
        assert!(!budget.take(now + Duration::from_secs(89), 2, cooldown));
        assert!(budget.take(now + cooldown, 2, cooldown));
        assert!(!budget.take(now + Duration::from_secs(9999), 2, cooldown));
    }
    #[test]
    fn renamed_and_disabled_services_and_tunnels_are_distinguished() {
        let text = "An asterisk denotes disabled.\n(1) Office WLAN\n(Hardware Port: Wi-Fi, Device: en0)\n(*) Disabled LAN\n(Hardware Port: Ethernet, Device: en2)\n(2) VPN\n(Hardware Port: VPN, Device: utun9)\n(3) USB LAN\n(Hardware Port: Ethernet, Device: en5)";
        assert_eq!(
            parse_service_order(text),
            vec![
                ("en0".into(), "Office WLAN".into()),
                ("en5".into(), "USB LAN".into())
            ]
        );
    }
}
