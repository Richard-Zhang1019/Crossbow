// 自动生成自 lib.rs 的 command 分域提取（#423）。use crate::* 引入根模块项。
use crate::*;
use std::{collections::BTreeSet, net::IpAddr};
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(serde::Serialize)]
pub(crate) struct LanAddress {
    interface: String,
    address: String,
}

#[tauri::command]
pub(crate) fn lan_addresses() -> Result<Vec<LanAddress>, String> {
    let interfaces = if_addrs::get_if_addrs().map_err(|error| error.to_string())?;
    Ok(interfaces
        .into_iter()
        .filter(if_addrs::Interface::is_oper_up)
        .filter_map(|interface| match interface.ip() {
            IpAddr::V4(address)
                if !address.is_unspecified()
                    && !address.is_loopback()
                    && !address.is_link_local()
                    && !address.is_multicast()
                    && !address.is_broadcast() =>
            {
                Some((interface.name, address))
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|(interface, address)| LanAddress {
            interface,
            address: address.to_string(),
        })
        .collect())
}

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
        store.transaction(|data| {
            data.ui.lang = lang.clone();
            Ok(())
        })?;
    }
    let _ = app.emit("ui://lang", lang);
    refresh_tray(&app);
    Ok(())
}
#[tauri::command]
pub(crate) fn set_lightweight_close(state: State<AppState>, enabled: bool) -> Result<(), String> {
    let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
    store.transaction(|data| {
        data.ui.lightweight_close = enabled;
        Ok(())
    })?;
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
    if store.data().ui.deep_link_import_confirm == enabled {
        return Ok(());
    }
    store.transaction(|data| {
        data.ui.deep_link_import_confirm = enabled;
        Ok(())
    })
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
        store.transaction(|data| {
            data.ui.theme = theme.clone();
            Ok(())
        })?;
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
        unified_delay: store.data().engine.unified_delay,
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
    store.transaction(|data| {
        data.engine.flag_emoji = enabled;
        Ok(())
    })?;
    Ok(())
}

#[tauri::command]
pub(crate) fn set_unified_delay(state: State<AppState>, enabled: bool) -> Result<(), String> {
    let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
    persist_unified_delay(&mut store, enabled)?;
    Ok(())
}

fn persist_unified_delay(store: &mut Store, enabled: bool) -> Result<(), String> {
    if store.data().engine.unified_delay == enabled {
        return Ok(());
    }
    store.transaction(|data| {
        data.engine.unified_delay = enabled;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_deep_link_opt_out_preserves_confirmation() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::open(directory.path()).unwrap();
        store.save().unwrap();
        let _inject =
            crossbow_core::secure_write::inject_path_at(&directory.path().join("store.json"), 2);
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

    #[test]
    fn failed_unified_delay_save_preserves_previous_setting() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::open(directory.path()).unwrap();
        store.save().unwrap();
        let _inject =
            crossbow_core::secure_write::inject_path_at(&directory.path().join("store.json"), 2);

        assert!(persist_unified_delay(&mut store, false).is_err());
        assert!(store.data().engine.unified_delay);
        assert!(
            Store::open(directory.path())
                .unwrap()
                .data()
                .engine
                .unified_delay
        );
    }
}

/// #598：读取 DNS 设置。
#[tauri::command]
pub(crate) fn get_dns_settings(state: State<'_, AppState>) -> DnsSettings {
    state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .dns
        .clone()
}

/// Validate, apply and persist once, so rollback owns the previous DNS snapshot.
#[tauri::command]
pub(crate) async fn set_dns_settings(
    state: State<'_, AppState>,
    settings: DnsSettings,
) -> Result<(), String> {
    commit_config_change_inner(
        &state,
        |data| {
            let use_overrides = !state.safe_mode.load(std::sync::atomic::Ordering::SeqCst);
            stage_dns_settings(data, settings, use_overrides)
        },
        false,
    )
}

fn stage_dns_settings(
    data: &mut crossbow_core::StoreSchema,
    settings: DnsSettings,
    use_overrides: bool,
) -> Result<(), String> {
    crossbow_core::dns_settings::validate_for_engine(&settings, data.engine.engine)?;
    let mut candidate = data.clone();
    candidate.dns = settings;
    // PersistOnly transitions also need preflight: invalid settings must not
    // break the next launch. No active profile is a valid initial setup state.
    if candidate.active_profile.is_some() {
        crossbow_core::render_config_with(&candidate, use_overrides)
            .map_err(|error| error.to_string())?;
    }
    data.dns = candidate.dns;
    Ok(())
}

#[cfg(test)]
mod dns_tests {
    use super::*;
    use crossbow_core::dns_settings::DnsMode;

    #[test]
    fn dns_preflight_rejects_native_json_without_mutating_saved_state() {
        let mut data = crossbow_core::StoreSchema::default();
        data.engine.engine = crossbow_core::Engine::SingBox;
        data.active_profile = Some("native".into());
        data.profiles.push(crossbow_core::Profile {
            id: "native".into(),
            name: "native".into(),
            kind: crossbow_core::ProfileKind::Local,
            url: None,
            content: r#"{"outbounds":[{"type":"direct","tag":"DIRECT"}]}"#.into(),
            update_interval_min: 0,
            override_ids: vec![],
            last_updated: None,
            traffic: None,
            last_update: None,
        });
        let before = serde_json::to_value(&data).unwrap();
        let settings = DnsSettings {
            mode: DnsMode::Custom,
            proxy_nameservers: vec!["1.1.1.1".into()],
            direct_nameservers: vec!["8.8.8.8".into()],
            bootstrap_nameservers: vec![],
        };
        assert!(stage_dns_settings(&mut data, settings, true).is_err());
        assert_eq!(serde_json::to_value(&data).unwrap(), before);
    }

    #[test]
    fn unsupported_dns_is_rejected_even_before_importing_a_profile() {
        let mut data = crossbow_core::StoreSchema::default();
        data.engine.engine = crossbow_core::Engine::SingBox;
        let before = data.dns.clone();
        let invalid = DnsSettings {
            mode: DnsMode::Custom,
            proxy_nameservers: vec!["1.1.1.1".into()],
            direct_nameservers: vec!["8.8.8.8".into()],
            bootstrap_nameservers: vec!["9.9.9.9".into()],
        };
        assert!(stage_dns_settings(&mut data, invalid, true).is_err());
        assert_eq!(data.dns, before);
    }
}
