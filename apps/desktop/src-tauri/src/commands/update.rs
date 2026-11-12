// 自动生成自 lib.rs 的 command 分域提取（#423）。use crate::* 引入根模块项。
use crate::*;
use tauri::{AppHandle, State};

#[tauri::command]
pub(crate) fn set_update_auto_download(
    state: State<AppState>,
    enabled: bool,
) -> Result<(), String> {
    {
        let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store.data_mut().update.auto_download = enabled;
        store.save().map_err(|e| e.to_string())?;
    }
    let mut u = state.update.lock().unwrap_or_else(|e| e.into_inner());
    u.auto_download = enabled;
    Ok(())
}
#[tauri::command]
pub(crate) fn set_update_auto_check(state: State<AppState>, enabled: bool) -> Result<(), String> {
    {
        let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store.data_mut().update.auto_check = enabled;
        store.save().map_err(|e| e.to_string())?;
    }
    let mut u = state.update.lock().unwrap_or_else(|e| e.into_inner());
    u.auto_check = enabled;
    Ok(())
}
#[tauri::command]
pub(crate) fn update_settings_get(state: State<AppState>) -> crossbow_core::model::UpdateSettings {
    state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .update
        .clone()
}
#[tauri::command]
pub(crate) async fn update_install(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || update_check::install(&app))
        .await
        .map_err(|error| error.to_string())?
}
#[tauri::command]
pub(crate) fn update_start_download(app: AppHandle) -> Result<(), String> {
    std::thread::spawn(move || {
        if let Err(e) = update_check::download_and_prepare(&app) {
            eprintln!("update download: {}", notifications::redact(&e));
        }
    });
    Ok(())
}
#[tauri::command]
pub(crate) async fn update_check_now(app: AppHandle) -> Result<Option<String>, String> {
    // 手动检查是用户显式动作：发现新版本停在「发现新版本」状态，
    // 等「下载」按钮确认后再下载。「自动下载」开关只作用于定时检查（#305）。
    tauri::async_runtime::spawn_blocking(move || update_check::check_once(&app, false, false))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) fn update_state_get(app: AppHandle) -> Option<update_check::UpdateInfo> {
    update_check::current(&app)
}
