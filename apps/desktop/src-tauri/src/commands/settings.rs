// 自动生成自 lib.rs 的 command 分域提取（#423）。use crate::* 引入根模块项。
use crate::*;
use tauri::{AppHandle, Emitter, Manager, State};

#[tauri::command]
pub(crate) fn autostart_set(app: AppHandle, enable: bool) -> Result<(), String> {
    let mgr = app.autolaunch();
    if enable {
        mgr.enable().map_err(|e| e.to_string())
    } else {
        mgr.disable().map_err(|e| e.to_string())
    }
}
#[tauri::command]
pub(crate) fn autostart_status(app: AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}
#[tauri::command]
pub(crate) fn set_lang(app: AppHandle, state: State<AppState>, lang: String) -> Result<(), String> {
    if !["zh", "en"].contains(&lang.as_str()) {
        return Err(format!("invalid lang: {lang}"));
    }
    {
        let mut store = state.store.lock().unwrap();
        store.data_mut().ui.lang = lang.clone();
        store.save().map_err(|e| e.to_string())?;
    }
    let _ = app.emit("ui://lang", lang);
    refresh_tray(&app);
    Ok(())
}
#[tauri::command]
pub(crate) fn set_lightweight_close(state: State<AppState>, enabled: bool) -> Result<(), String> {
    let mut store = state.store.lock().unwrap();
    store.data_mut().ui.lightweight_close = enabled;
    store.save().map_err(|e| e.to_string())?;
    Ok(())
}
#[tauri::command]
pub(crate) fn set_theme(
    app: AppHandle,
    state: State<AppState>,
    theme: String,
) -> Result<(), String> {
    if !["system", "light", "dark"].contains(&theme.as_str()) {
        return Err(format!("invalid theme: {theme}"));
    }
    {
        let mut store = state.store.lock().unwrap();
        store.data_mut().ui.theme = theme.clone();
        store.save().map_err(|e| e.to_string())?;
    }
    if let Some(win) = app.get_webview_window("main") {
        apply_window_appearance(&win, &theme);
    }
    let _ = app.emit("ui://theme", theme);
    Ok(())
}
#[tauri::command]
pub(crate) fn get_engine_config(state: State<AppState>) -> EngineView {
    let store = state.store.lock().unwrap();
    let need = match store.data().engine.engine {
        crossbow_core::Engine::SingBox => "sing-box",
        crossbow_core::Engine::Mihomo => "mihomo",
    };
    let core_installed = crate::engine::download::resolve_core_for(&state.data_dir, Some(need))
        .map(|p| crate::engine::download::is_core_bin(&p, need))
        .unwrap_or(false);
    EngineView {
        engine: format!("{:?}", store.data().engine.engine).to_lowercase(),
        mixed_port: store.data().engine.mixed_port,
        allow_lan: store.data().engine.allow_lan,
        flag_emoji: store.data().engine.flag_emoji,
        linux_wireguard_coexist: store.data().engine.linux_wireguard_coexist,
        core_installed,
    }
}
#[tauri::command]
pub(crate) fn get_ui_settings(state: State<AppState>) -> UiSettings {
    state.store.lock().unwrap().data().ui.clone()
}
#[tauri::command]
pub(crate) fn platform_capabilities() -> platform::Capabilities {
    platform::capabilities()
}
#[tauri::command]
pub(crate) fn set_flag_emoji(state: State<AppState>, enabled: bool) -> Result<(), String> {
    let mut store = state.store.lock().unwrap();
    store.data_mut().engine.flag_emoji = enabled;
    store.save().map_err(|e| e.to_string())?;
    Ok(())
}
