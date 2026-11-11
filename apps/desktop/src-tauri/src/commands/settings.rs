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
        let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store.data_mut().ui.lang = lang.clone();
        store.save().map_err(|e| e.to_string())?;
    }
    let _ = app.emit("ui://lang", lang);
    refresh_tray(&app);
    Ok(())
}
#[tauri::command]
pub(crate) fn set_lightweight_close(state: State<AppState>, enabled: bool) -> Result<(), String> {
    let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
    store.data_mut().ui.lightweight_close = enabled;
    store.save().map_err(|e| e.to_string())?;
    Ok(())
}
#[tauri::command]
pub(crate) fn set_deep_link_import_confirm(
    state: State<AppState>,
    enabled: bool,
) -> Result<(), String> {
    let mut store = state.store.lock().map_err(|e| e.to_string())?;
    persist_deep_link_import_confirm(&mut store, enabled)
}

fn persist_deep_link_import_confirm(store: &mut Store, enabled: bool) -> Result<(), String> {
    let previous = store.data().ui.deep_link_import_confirm;
    if previous == enabled {
        return Ok(());
    }
    store.data_mut().ui.deep_link_import_confirm = enabled;
    if let Err(error) = store.save() {
        store.data_mut().ui.deep_link_import_confirm = previous;
        return Err(error.to_string());
    }
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
        let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
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
    let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
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
    state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .ui
        .clone()
}
#[tauri::command]
pub(crate) fn platform_capabilities() -> platform::Capabilities {
    platform::capabilities()
}
#[tauri::command]
pub(crate) fn set_flag_emoji(state: State<AppState>, enabled: bool) -> Result<(), String> {
    let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
    store.data_mut().engine.flag_emoji = enabled;
    store.save().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_deep_link_opt_out_preserves_confirmation() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::open(directory.path()).unwrap();
        store.save().unwrap();
        std::fs::create_dir(directory.path().join("store.json.tmp")).unwrap();
        assert!(persist_deep_link_import_confirm(&mut store, false).is_err());
        assert!(store.data().ui.deep_link_import_confirm);
        assert!(
            Store::open(directory.path())
                .unwrap()
                .data()
                .ui
                .deep_link_import_confirm
        );
    }
}
