// 自动生成自 lib.rs 的 command 分域提取（#423）。use crate::* 引入根模块项。
use crate::*;
use tauri::{AppHandle, Emitter, Manager, State};

#[tauri::command]
pub(crate) fn traffic_history(state: State<AppState>) -> Vec<TrafficHistoryEntry> {
    state
        .traffic_history_snapshot()
        .into_iter()
        .map(|sample| TrafficHistoryEntry {
            up: sample.up,
            down: sample.down,
        })
        .collect()
}
#[tauri::command]
pub(crate) fn open_logs_dir(state: State<AppState>) -> Result<(), String> {
    let dir = crate::diagnostics::log_files::logs_dir(&state.data_dir);
    let _ = std::fs::create_dir_all(&dir);
    #[cfg(target_os = "macos")]
    {
        crate::process::command("open")
            .arg(&dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(windows)]
    {
        crate::process::command("explorer")
            .arg(&dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
#[tauri::command]
pub(crate) fn set_logs_retention(state: State<AppState>, days: u32) -> Result<(), String> {
    if ![1, 3, 5, 7, 15, 30].contains(&days) {
        return Err(format!("invalid retention days: {days}"));
    }
    {
        let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store.data_mut().logs.retention_days = days;
        store.save().map_err(|e| e.to_string())?;
    }
    // 立即按新保留期清理一次
    let dir = crate::diagnostics::log_files::logs_dir(&state.data_dir);
    crate::diagnostics::log_files::sweep(&dir, days);
    Ok(())
}
#[tauri::command]
pub(crate) fn set_logs_save(
    app: AppHandle,
    state: State<AppState>,
    enabled: bool,
) -> Result<(), String> {
    {
        let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store.data_mut().logs.save_enabled = enabled;
        store.save().map_err(|e| e.to_string())?;
    }
    update_logs_persistence(&app);
    Ok(())
}
/// 日志保存设置（#29）。
#[tauri::command]
pub(crate) fn logs_settings(state: State<AppState>) -> crossbow_core::model::LogSettings {
    state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .logs
        .clone()
}
/// 前端黑匣子：全局错误/心跳落盘，用于定位 webview 白屏类问题。
#[tauri::command]
pub(crate) fn diag_log(state: State<AppState>, line: String) {
    use std::io::Write;
    if let Ok(mut f) =
        crate::platform::fs_perms::open_private_append(&state.data_dir.join("webview.log"))
    {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "{ts} {line}");
    }
}
#[tauri::command]
pub(crate) fn close_connection(state: State<AppState>, id: String) -> Result<(), String> {
    controller_client(&state)?.close_connection(&id)
}
#[tauri::command]
pub(crate) fn current_outbound(
    state: State<AppState>,
) -> Result<crate::engine::api::OutboundInfo, String> {
    let fallback = state
        .core()
        .current_config()
        .and_then(|c| extract_final_outbound(&c));
    let c = controller_client(&state)?;
    c.current_outbound(fallback.as_deref())
}
#[tauri::command]
pub(crate) async fn run_diagnosis(
    app: AppHandle,
    run_id: String,
) -> Result<Vec<crate::diagnostics::diagnose::CheckResult>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        crate::diagnostics::diagnose::run(&state, &run_id, |progress| {
            let _ = app.emit("diagnosis://progress", progress);
        })
    })
    .await
    .map_err(|e| e.to_string())
}
#[tauri::command]
pub(crate) fn unsubscribe_logs(state: State<AppState>, subscriber: String) -> Result<(), String> {
    state.ws.end_log_view(&subscriber);
    Ok(())
}
#[tauri::command]
pub(crate) fn clear_logs(state: State<AppState>) -> crate::diagnostics::log_cache::Snapshot {
    state.ws.clear_logs()
}
#[tauri::command]
pub(crate) fn subscribe_logs(
    app: AppHandle,
    state: State<AppState>,
    subscriber: String,
) -> Result<crate::diagnostics::log_cache::Snapshot, String> {
    state.ws.begin_log_view(subscriber);
    if let Some((port, secret)) = state.core().controller() {
        let h = app.clone();
        state
            .ws
            .subscribe_logs(port, &secret, move |ev, payload: serde_json::Value| {
                let _ = h.emit(ev, payload);
            })?;
    }
    Ok(state.ws.logs_snapshot())
}
#[tauri::command]
pub(crate) fn unsubscribe_connections(state: State<AppState>) -> Result<(), String> {
    state.ws.unsubscribe("connections");
    Ok(())
}
#[tauri::command]
pub(crate) fn subscribe_connections(
    app: AppHandle,
    state: State<AppState>,
    include_processes: Option<bool>,
) -> Result<(), String> {
    state
        .ws
        .want_connections(include_processes.unwrap_or(false));
    let (port, secret) = require_controller(&state)?;
    let h = app.clone();
    state
        .ws
        .subscribe_connections(port, &secret, move |ev, payload: serde_json::Value| {
            let _ = h.emit(ev, payload);
        })
}
#[tauri::command]
pub(crate) fn unsubscribe_traffic(state: State<AppState>) -> Result<(), String> {
    state.ws.unsubscribe("traffic");
    Ok(())
}
#[tauri::command]
pub(crate) fn subscribe_traffic(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    state.ws.want_channel("traffic");
    let (port, secret) = require_controller(&state)?;
    let h = app.clone();
    state
        .ws
        .subscribe_traffic(port, &secret, move |ev, payload| {
            let _ = h.emit(ev, payload);
        })
}
