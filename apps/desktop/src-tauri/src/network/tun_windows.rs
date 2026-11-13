//! Windows TUN 切换（#81）。
//!
//! 内核创建 wintun 适配器需要管理员权限，GUI 进程无从提权——TUN 内核由
//! 特权服务（crossbow-service，NSIS 注册开机自启）以 SYSTEM 拉起；端口/
//! 密钥生成、配置注入、就绪探测与状态簿记复用
//! `CoreManager::start_with_spawner`。与 macOS 版 toggle_tun 相比没有
//! setuid/授权副本概念：服务装好即授权持久化。

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use crate::{tray::wiring::HealthRecovery, AppState};

pub fn toggle_tun(state: &AppState, enable: bool) -> Result<(), String> {
    // 与 macOS 相同：整段切换与启动/健康监测串行化，存储标志不得跑在
    // 失败的停止前面。
    let _operation = state.core_op.lock().unwrap_or_else(|e| e.into_inner());
    state.exit.ensure_open()?;
    let _recovery =
        crate::engine::config_transition::RecoveryOwner::enter(&state.config_transition);
    let result = toggle_tun_locked(state, enable, true);
    publish_terminal_core_error(state);
    result
}

/// Keep the restart and its fallback in the same transaction. An old probe
/// cannot stop a new core installed by a manual switch.
pub(crate) fn recover_for_health(
    state: &AppState,
    expected_pid: Option<u32>,
    expected_status: &crate::CoreStatus,
    may_restart: bool,
) -> Result<HealthRecovery, String> {
    let _operation = state.core_op.lock().unwrap_or_else(|e| e.into_inner());
    state.exit.ensure_open()?;
    if !state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .engine
        .tun_enable
        || state.core().pid() != expected_pid
        || &state.core().status() != expected_status
    {
        return Ok(HealthRecovery::Superseded);
    }
    let _recovery =
        crate::engine::config_transition::RecoveryOwner::enter(&state.config_transition);
    if may_restart && *expected_status == crate::CoreStatus::Running {
        let restart = if state.core().stop() {
            start_and_wait(state, true).and_then(|()| crate::tray::wiring::probe_tun_network())
        } else {
            Err("无法停止 TUN 内核".into())
        };
        match restart {
            Ok(()) => return Ok(HealthRecovery::Restarted),
            Err(error) => eprintln!("tun health: restart failed; disabling TUN: {error}"),
        }
    }
    let result = toggle_tun_locked(state, false, false);
    publish_terminal_core_error(state);
    result?;
    Ok(HealthRecovery::Disabled)
}

fn publish_terminal_core_error(state: &AppState) {
    let status = state.core().status();
    if matches!(status, crate::CoreStatus::Crashed(_)) {
        // Crash side effects are suppressed while the transition owns recovery.
        // Send the final state so the frontend can leave its Starting state.
        use tauri::Emitter;
        let _ = state.app.emit("core://status", status);
    }
}

fn toggle_tun_locked(
    state: &AppState,
    enable: bool,
    restore_previous_on_failure: bool,
) -> Result<(), String> {
    let previous_tun = {
        let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store.data().engine.tun_enable
    };
    if previous_tun == enable {
        return Ok(());
    }
    let was_running = matches!(
        state.core().status(),
        crate::CoreStatus::Running | crate::CoreStatus::Starting
    );
    if enable {
        if !crate::service_client::installed() {
            crate::service_client::install_elevated()?;
        }
        crate::service_client::running()?;
    }
    let previous_proxy = state.sysproxy.status();
    let suspended_proxy = if enable && !previous_proxy.enabled {
        Some(state.sysproxy.suspend_for_tun()?)
    } else {
        None
    };

    // 与 macOS 相同：先停现有内核再翻转。CoreManager 的启动预检要求
    // Stopped/Crashed，直接 start 会报「core is Running, stop it first」
    // （v0.11.0 实测，#159 用户反馈）。服务托管的 SYSTEM 内核由 stop()
    // 内部路由给特权服务停止，用户态内核走常规路径。
    if (was_running || state.core().pid().is_some()) && !state.core().stop() {
        let proxy_error = suspended_proxy
            .and_then(|suspended| state.sysproxy.restore_after_failed_tun(suspended).err());
        let proxy_error = proxy_error.or_else(|| {
            if previous_proxy.enabled {
                state.sysproxy.disable().err()
            } else {
                None
            }
        });
        return Err(format!(
            "旧内核未退出，TUN 状态未改变；请重试切换{}",
            proxy_error
                .map(|error| format!("；系统代理恢复失败：{error}"))
                .unwrap_or_default()
        ));
    }

    {
        let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store.data_mut().engine.tun_enable = enable;
        if let Err(error) = store.save() {
            store.data_mut().engine.tun_enable = previous_tun;
            drop(store);
            let mut errors = Vec::new();
            if restore_previous_on_failure {
                restore_previous_session(
                    state,
                    was_running || previous_proxy.enabled,
                    previous_tun,
                    previous_proxy.enabled,
                    suspended_proxy,
                    &mut errors,
                );
            } else if let Err(error) = restore_proxy_without_core(state) {
                errors.push(format!("系统代理恢复失败：{error}"));
            }
            let message = format!("保存 TUN 设置失败：{error}");
            return Err(if restore_previous_on_failure {
                describe_rollback(message, errors)
            } else if errors.is_empty() {
                format!("{message}；故障内核已停止，TUN 设置未能关闭")
            } else {
                format!("{message}；{}", errors.join("；"))
            });
        }
    }

    // 开 TUN 必须有内核；关 TUN 时若原本有内核，重启为非 TUN 模式。
    // 内核的拉起/停止在 start_core_locked 内部分流到服务（SYSTEM）。
    let should_restart = was_running || enable || previous_tun;
    if should_restart {
        let start_result = start_and_wait(state, enable);
        if let Err(start_error) = start_result {
            if !restore_previous_on_failure {
                let stopped = state.core().stop();
                let proxy_result = restore_proxy_without_core(state);
                let stop_error = if stopped {
                    ""
                } else {
                    "；未能确认失败的内核已停止"
                };
                return Err(format!(
                    "TUN 已关闭，但普通内核启动失败：{start_error}{stop_error}{}",
                    proxy_result
                        .err()
                        .map(|error| format!("；系统代理恢复失败：{error}"))
                        .unwrap_or_default()
                ));
            }
            let settings_error = {
                let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
                store.data_mut().engine.tun_enable = previous_tun;
                store.save().err().map(|error| error.to_string())
            };
            let mut recovery_errors = Vec::new();
            if let Some(error) = settings_error {
                recovery_errors.push(format!("保存旧 TUN 设置失败：{error}"));
            }
            restore_previous_session(
                state,
                was_running || previous_proxy.enabled,
                previous_tun,
                previous_proxy.enabled,
                suspended_proxy,
                &mut recovery_errors,
            );
            return Err(describe_rollback(
                format!("TUN 切换失败：{start_error}"),
                recovery_errors,
            ));
        }
    }
    if !enable {
        state
            .sysproxy
            .restore_after_tun()
            .map_err(|error| format!("TUN 已关闭，但系统代理恢复失败：{error}"))?;
    }
    Ok(())
}

fn describe_rollback(message: String, errors: Vec<String>) -> String {
    if errors.is_empty() {
        format!("{message}；旧状态已恢复")
    } else {
        format!("{message}；恢复未完成：{}", errors.join("；"))
    }
}

fn restore_previous_session(
    state: &AppState,
    should_run: bool,
    previous_tun: bool,
    proxy_was_enabled: bool,
    suspended_proxy: Option<super::sysproxy::SuspendedSysProxy>,
    errors: &mut Vec<String>,
) {
    let recovery = restore_previous_core(state, should_run, previous_tun);
    let proxy_error = if proxy_was_enabled && recovery.is_err() {
        state.sysproxy.disable().err()
    } else {
        suspended_proxy.and_then(|snapshot| state.sysproxy.restore_after_failed_tun(snapshot).err())
    };
    if let Err(error) = recovery {
        errors.push(format!("旧内核恢复失败：{error}"));
    }
    if let Some(error) = proxy_error {
        errors.push(format!("系统代理恢复失败：{error}"));
    }
}

fn restore_proxy_without_core(state: &AppState) -> Result<(), String> {
    state
        .sysproxy
        .disable()
        .and_then(|()| state.sysproxy.recover_tun_without_core())
}

fn restore_previous_core(
    state: &AppState,
    should_run: bool,
    previous_tun: bool,
) -> Result<(), String> {
    if (state.core().pid().is_some()
        || matches!(
            state.core().status(),
            crate::CoreStatus::Running | crate::CoreStatus::Starting
        ))
        && !state.core().stop()
    {
        return Err("未能停止失败的内核".into());
    }
    if !should_run {
        return Ok(());
    }
    if let Err(error) = start_and_wait(state, previous_tun) {
        let cleanup = if state.core().pid().is_some() && !state.core().stop() {
            "；未能确认恢复中的内核已停止"
        } else {
            ""
        };
        return Err(format!("{error}{cleanup}"));
    }
    Ok(())
}

fn start_and_wait(state: &AppState, tun_enabled: bool) -> Result<(), String> {
    crate::start_core_locked(state)?;
    if tun_enabled {
        wait_tun_active(state)?;
    } else {
        crate::wait_core_running(state)?;
    }
    verify_mixed_listener(state)
}

fn verify_mixed_listener(state: &AppState) -> Result<(), String> {
    let port = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .engine
        .mixed_port;
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&address, Duration::from_millis(250))
        .map(|_| ())
        .map_err(|error| format!("混合端口 {port} 未就绪：{error}"))
}

/// 就绪判定：控制器报告 tun 已启用且适配器在位（wintun 建立通常 <1s，
/// 服务拉起内核 + 探测窗口兜底 15s）。
fn wait_tun_active(state: &AppState) -> Result<(), String> {
    let deadline = Instant::now() + crate::engine::timing::WINDOWS_TUN_READY_DEADLINE;
    loop {
        match state.core().status() {
            crate::CoreStatus::Running => {
                if let Some((port, secret)) = state.core().controller() {
                    let client = crate::engine::api::Controller { port, secret };
                    if let Ok(Some(device)) = client.active_tun_device() {
                        if adapter_captures_public_ipv4(&device) == Some(true) {
                            return Ok(());
                        }
                    }
                }
            }
            crate::CoreStatus::Crashed(error) => return Err(error),
            crate::CoreStatus::Stopped => return Err("TUN 内核已停止".into()),
            crate::CoreStatus::Starting => {}
        }
        if Instant::now() >= deadline {
            return Err("TUN 适配器未在预期时间内建立（查看内核日志获取详情）".into());
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// 兼容 macOS toggle 的路由预检签名；Windows 无 netstat 分流路由概念，
/// 冲突检测交给 coexist 的适配器识别（#82），此处恒通过。
pub fn ensure_route_available() -> Result<BTreeSet<(String, String)>, String> {
    Ok(BTreeSet::new())
}

/// Verify the named adapter is actually up using the operating system.
pub(crate) fn diagnostic_adapter_ready(device: &str) -> Option<bool> {
    adapter_index(device).map(|index| index.is_some())
}

fn adapter_captures_public_ipv4(device: &str) -> Option<bool> {
    use windows_sys::Win32::NetworkManagement::IpHelper::GetBestInterface;
    let Some(adapter) = adapter_index(device)? else {
        return Some(false);
    };
    // Use a public destination outside Crossbow's private route exclusions.
    // IP Helper reads the routing table without sending a packet.
    let mut selected = 0;
    let destination = u32::from_ne_bytes([1, 1, 1, 1]);
    let status = unsafe { GetBestInterface(destination, &mut selected) };
    (status == 0).then_some(selected == adapter)
}

fn adapter_index(device: &str) -> Option<Option<u32>> {
    use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    use windows_sys::Win32::NetworkManagement::{
        IpHelper::{
            ConvertInterfaceAliasToLuid, GetIfEntry2Ex, MibIfEntryNormalWithoutStatistics,
            MIB_IF_ROW2,
        },
        Ndis::IfOperStatusUp,
    };
    let alias: Vec<_> = device.encode_utf16().chain(Some(0)).collect();
    // MIB_IF_ROW2 is a plain Win32 output structure. Alias lookup supplies the input LUID.
    let mut row: MIB_IF_ROW2 = unsafe { std::mem::zeroed() };
    let status = unsafe { ConvertInterfaceAliasToLuid(alias.as_ptr(), &mut row.InterfaceLuid) };
    if status == ERROR_FILE_NOT_FOUND {
        return Some(None);
    }
    if status != 0 {
        return None;
    }
    let status = unsafe { GetIfEntry2Ex(MibIfEntryNormalWithoutStatistics, &mut row) };
    (status == 0).then_some((row.OperStatus == IfOperStatusUp).then_some(row.InterfaceIndex))
}
