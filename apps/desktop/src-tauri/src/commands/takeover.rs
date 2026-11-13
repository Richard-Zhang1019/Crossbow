// 自动生成自 lib.rs 的 command 分域提取（#423）。use crate::* 引入根模块项。
use crate::*;
use tauri::{AppHandle, Emitter, Manager, State};

#[tauri::command]
pub(crate) async fn sysproxy_toggle(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    notifications::publish(
        &app,
        notifications::Notice::new(notifications::Kind::Progress, "tray.proxy", "system-proxy"),
    );
    let result = toggle_sysproxy_state(&state);
    let _ = app.emit("sysproxy://status", state.sysproxy.status());
    refresh_tray(&app);
    publish_proxy_toggle_result(&app, "system-proxy", result.clone());
    result
}
#[tauri::command]
pub(crate) async fn diagnosis_enable_sysproxy(app: AppHandle) -> Result<(), String> {
    let handle = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<AppState>();
        let _operation = state.core_op.lock().unwrap_or_else(|e| e.into_inner());
        state.exit.ensure_open()?;
        if !platform::capabilities().system_proxy {
            return Err("当前平台不支持系统代理接管".into());
        }
        let (port, tun) = {
            let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
            (
                store.data().engine.mixed_port,
                store.data().engine.tun_enable,
            )
        };
        if tun || state.core().status() != CoreStatus::Running {
            return Err("接管状态已变化，请重新诊断".into());
        }
        let was_enabled = state.sysproxy.status().enabled;
        state.sysproxy.enable_from_diagnosis(port)?;
        if !was_enabled
            && state.sysproxy.status().enabled
            && state.core().status() != CoreStatus::Running
        {
            state
                .sysproxy
                .disable()
                .map_err(|e| format!("内核已停止，代理恢复失败：{e}"))?;
            return Err("内核已停止，系统代理已恢复".into());
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|result| result);
    let state = app.state::<AppState>();
    let _ = app.emit("sysproxy://status", state.sysproxy.status());
    refresh_tray(&app);
    notifications::result(&app, "diagnostics-proxy", "tray.proxy", result.clone());
    result
}

#[tauri::command]
pub(crate) fn sysproxy_status(state: State<AppState>) -> SysProxyStatus {
    state.sysproxy.status()
}
#[tauri::command]
pub(crate) async fn set_allow_lan(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    commit_config_change(&state, |data| {
        data.engine.allow_lan = enabled;
        Ok(())
    })?;
    Ok(())
}
/// 修改混合端口：持久化 → 运行中则热重启 → 系统代理开启则跟随换端口。
#[tauri::command]
pub(crate) async fn set_mixed_port(
    app: AppHandle,
    state: State<'_, AppState>,
    port: u16,
) -> Result<(), String> {
    if port < 1024 {
        return Err("端口不能小于 1024".into());
    }
    commit_config_change(&state, |data| {
        data.engine.mixed_port = port;
        Ok(())
    })?;
    let _ = app.emit("sysproxy://status", state.sysproxy.status());
    Ok(())
}
#[tauri::command]
pub(crate) async fn coexist_detect(app: AppHandle) -> Vec<crate::network::coexist::CoexistFinding> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::network::coexist::detect(&app.state::<AppState>())
    })
    .await
    .unwrap_or_default()
}
#[tauri::command]
pub(crate) async fn test_site(
    app: AppHandle,
    id: String,
) -> Result<crate::network::site_tests::SiteResult, String> {
    let (port, running) = {
        let state = app.state::<AppState>();
        let port = state
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .data()
            .engine
            .mixed_port;
        (port, state.core().status() == CoreStatus::Running)
    };
    tauri::async_runtime::spawn_blocking(move || {
        crate::network::site_tests::probe(&id, port, running)
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) async fn set_linux_wireguard_coexist(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    let result = crate::network::tun_linux::set_wireguard_coexist(&state, enabled);
    #[cfg(not(target_os = "linux"))]
    let result: Result<(), String> = {
        let _ = (state, enabled);
        Err("当前平台不支持 Linux WireGuard 共存设置".into())
    };
    const OPERATION: &str = "linux-wireguard-coexist";
    match &result {
        Ok(()) => notifications::resolve(&app, OPERATION),
        Err(error) => notifications::result(
            &app,
            OPERATION,
            "linux.wireguardCoexist",
            Err(error.clone()),
        ),
    }
    result
}
#[tauri::command]
pub(crate) fn tun_status(state: State<AppState>) -> TunStatus {
    #[cfg(target_os = "macos")]
    {
        let enabled = {
            let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
            store.data().engine.tun_enable
        };
        let is_singbox = {
            let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
            store.data().engine.engine == crossbow_core::Engine::SingBox
        };
        let need = if is_singbox { "sing-box" } else { "mihomo" };
        // 授权副本在 tun-binaries/<版本>/ 下（PR13），检查它的 setuid 位
        let ready = crate::network::tun::authorized_copy_exists(&state.data_dir, need);
        TunStatus {
            enabled,
            ready,
            core_running: state.core().status() == CoreStatus::Running,
        }
    }
    #[cfg(target_os = "windows")]
    {
        let enabled = {
            let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
            store.data().engine.tun_enable
        };
        TunStatus {
            enabled,
            // 服务随安装器注册即视为就绪（GUI 无需再授权）
            ready: crate::service_client::installed(),
            core_running: state.core().status() == CoreStatus::Running,
        }
    }
    #[cfg(target_os = "linux")]
    {
        TunStatus {
            enabled: crate::network::linux_service::status()
                .map(|s| s.is_some_and(|s| s.active))
                .unwrap_or_else(|_| {
                    state
                        .store
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .data()
                        .engine
                        .tun_enable
                }),
            ready: crate::network::linux_service::installed(),
            core_running: state.core().status() == CoreStatus::Running,
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        TunStatus {
            enabled: false,
            ready: false,
            core_running: false,
        }
    }
}
/// TUN 开关（macOS）；系统代理有独立开关，可共用同一个内核。
#[tauri::command]
pub(crate) async fn set_tun(
    app: AppHandle,
    state: State<'_, AppState>,
    enable: bool,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        notifications::publish(
            &app,
            notifications::Notice::new(notifications::Kind::Progress, "tray.tun", "tun-switch"),
        );
        let result = crate::network::tun::toggle_tun(&state, enable);
        notifications::tun_result(&app, &state, "tun-switch", "tray.tun", result.clone());
        let actual = state
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .data()
            .engine
            .tun_enable;
        let _ = app.emit("tun://status", actual);
        let _ = app.emit("sysproxy://status", state.sysproxy.status());
        refresh_tray(&app);
        result
    }
    #[cfg(target_os = "windows")]
    {
        notifications::publish(
            &app,
            notifications::Notice::new(notifications::Kind::Progress, "tray.tun", "tun-switch"),
        );
        let result = crate::network::tun_windows::toggle_tun(&state, enable);
        notifications::tun_result(&app, &state, "tun-switch", "tray.tun", result.clone());
        let actual = state
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .data()
            .engine
            .tun_enable;
        let _ = app.emit("tun://status", actual);
        let _ = app.emit("sysproxy://status", state.sysproxy.status());
        refresh_tray(&app);
        result
    }
    #[cfg(target_os = "linux")]
    {
        let result = crate::network::tun_linux::toggle_tun(&state, enable);
        notifications::result(&app, "tun-switch", "tray.tun", result.clone());
        let actual = crate::network::linux_service::status()
            .map(|s| s.is_some_and(|s| s.active))
            .unwrap_or_else(|_| {
                state
                    .store
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .data()
                    .engine
                    .tun_enable
            });
        let _ = app.emit("tun://status", actual);
        refresh_tray(&app);
        result
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = enable;
        let result = Err("当前平台暂不支持 TUN，请使用显式 HTTP/SOCKS 代理".into());
        notifications::result(&app, "tun-switch", "tray.tun", result.clone());
        result
    }
}
