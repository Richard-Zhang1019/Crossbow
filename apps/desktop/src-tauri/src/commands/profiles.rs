// 自动生成自 lib.rs 的 command 分域提取（#423）。use crate::* 引入根模块项。
use crate::*;
use tauri::{AppHandle, Emitter, State};

#[tauri::command]
pub(crate) fn active_profile_id(state: State<AppState>) -> Option<String> {
    state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .active_profile
        .clone()
}
#[tauri::command]
pub(crate) fn list_profiles(state: State<AppState>) -> Vec<Profile> {
    state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .profiles
        .clone()
}
#[tauri::command]
pub(crate) fn remove_profile(
    app: AppHandle,
    state: State<AppState>,
    id: String,
) -> Result<(), String> {
    {
        let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store.transaction(|data| data.remove_profile(&id))?;
    }
    refresh_tray(&app);
    Ok(())
}
#[tauri::command]
pub(crate) async fn set_active_profile(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    commit_config_change(&state, |data| data.set_active_profile(&id))?;
    refresh_tray(&app);
    // 托盘/前端切换共用此命令：发事件让前端各处（页头配置名、概览卡片等）
    // 同步刷新——切换器自己的 DOM 事件只有页面内组件收得到（#357）。
    let _ = app.emit("profile://switched", id.as_str());
    Ok(())
}
/// 设置订阅自动更新间隔（分钟，0 = 关闭）。
#[tauri::command]
pub(crate) fn set_profile_interval(
    state: State<AppState>,
    id: String,
    interval_min: u32,
) -> Result<(), String> {
    let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
    store.transaction(|data| {
        let p = data
            .profiles
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or("profile not found")?;
        p.update_interval_min = interval_min;
        Ok(())
    })?;
    scheduler::clear_retry(&id);
    Ok(())
}
/// 重命名 Profile。
#[tauri::command]
pub(crate) fn rename_profile(
    app: AppHandle,
    state: State<AppState>,
    id: String,
    name: String,
) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("名称不能为空".into());
    }
    {
        let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store.transaction(|data| {
            let p = data
                .profiles
                .iter_mut()
                .find(|p| p.id == id)
                .ok_or("profile not found")?;
            p.name = name.to_string();
            Ok(())
        })?;
    }
    refresh_tray(&app);
    Ok(())
}
/// 刷新订阅并落库；正在运行则热重启应用新配置。
#[tauri::command]
pub(crate) async fn update_profile(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<Profile, String> {
    let profile = refresh_profile_blocking(&state, &id)?;
    scheduler::clear_retry(&id);
    notifications::resolve(&app, &format!("profile-auto-update-{id}"));
    refresh_tray(&app);
    Ok(profile)
}
/// 导入本地配置文件（文件选择器与拖拽导入共用）。
#[tauri::command]
pub(crate) async fn import_profile_file(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<Profile, String> {
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("读取文件失败 {path}: {e}"))?;
    let profile = import_content_blocking(&state, profile_name_from_path(&path), content)
        .map_err(|e| format!("{path}: {e}"))?;
    refresh_tray(&app);
    Ok(profile)
}
/// 导入本地/粘贴的配置内容。
#[tauri::command]
pub(crate) async fn import_profile_content(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    content: String,
) -> Result<Profile, String> {
    let profile = import_content_blocking(&state, name, content)?;
    refresh_tray(&app);
    Ok(profile)
}
/// 导入 URL 订阅：拉取 → 校验 → 落库。
#[tauri::command]
pub(crate) async fn import_profile_url(
    app: AppHandle,
    state: State<'_, AppState>,
    url: String,
    name: Option<String>,
) -> Result<Profile, String> {
    let profile = import_url_blocking(&state, &url, name.as_deref())?;
    refresh_tray(&app);
    Ok(profile)
}

/// Read-only, local inspection. Never returns input, nodes, hosts or credentials.
#[derive(serde::Serialize)]
pub(crate) struct ImportInspection {
    engine: crossbow_core::Engine,
    tested_version: String,
    status: &'static str,
    node_count: usize,
    protocols: Vec<String>,
    capabilities: Vec<ImportCapability>,
}
#[derive(serde::Serialize)]
pub(crate) struct ImportCapability {
    protocol: &'static str,
    mihomo: &'static str,
    singbox: &'static str,
}
#[tauri::command]
pub(crate) async fn inspect_profile_import(
    state: State<'_, AppState>,
    content: String,
) -> Result<ImportInspection, String> {
    let data = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .clone();
    let version = format!(
        "mihomo v1.19.32 / sing-box v{}",
        crossbow_core::singbox_convert::SINGBOX_VERSION
    );
    tauri::async_runtime::spawn_blocking(move || inspect_import(&data, &content, version))
        .await
        .map_err(|_| "导入检查任务失败".to_string())
}
fn inspect_import(
    data: &crossbow_core::StoreSchema,
    content: &str,
    version: String,
) -> ImportInspection {
    let mut report = ImportInspection {
        engine: data.engine.engine,
        tested_version: version,
        status: "empty",
        node_count: 0,
        protocols: vec![],
        capabilities: vec![
            ImportCapability {
                protocol: "hy2",
                mihomo: "subset",
                singbox: "limited",
            },
            ImportCapability {
                protocol: "tuic",
                mihomo: "subset",
                singbox: "unsupported",
            },
            ImportCapability {
                protocol: "reality",
                mihomo: "subset",
                singbox: "unsupported",
            },
        ],
    };
    let input = content.trim();
    if input.len() > 8 * 1024 * 1024 {
        report.status = "invalid";
        return report;
    }
    if input.is_empty() {
        return report;
    }
    if input.lines().count() == 1 && (input.starts_with("https://") || input.starts_with("http://"))
    {
        report.status = "remote";
        return report;
    }
    let Ok(normalized) = crossbow_core::subscription::normalize_profile_content(input) else {
        report.status = "invalid";
        return report;
    };
    let document: serde_json::Value = serde_yaml::from_str(&normalized).unwrap_or_default();
    if let Some(nodes) = document
        .get("proxies")
        .or_else(|| document.get("outbounds"))
        .and_then(serde_json::Value::as_array)
    {
        report.node_count = nodes.len();
        for node in nodes {
            let protocol = node
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("other");
            let protocol = match protocol {
                "hysteria2" => "hy2",
                "vless" if node.get("reality-opts").is_some() => "reality",
                "ss" | "vmess" | "trojan" | "vless" | "tuic" => protocol,
                _ => "other",
            };
            if !report.protocols.iter().any(|p| p == protocol) {
                report.protocols.push(protocol.into());
            }
        }
    }
    let mut profile = make_profile(
        gen_id(),
        "Compatibility preview".into(),
        ProfileKind::Local,
        None,
    );
    profile.content = normalized;
    report.status = if validate_import_candidate(data, &profile).is_ok() {
        "compatible"
    } else {
        "unsupported"
    };
    report
}

#[cfg(test)]
mod import_inspection_tests {
    use super::*;
    #[test]
    fn capability_rows_agree_with_actual_conversion_and_do_not_return_secrets() {
        for (uri, protocol, singbox) in [
            ("hy2://PRIVATE_AUTH@node.test:443", "hy2", true),
            ("hy2://PRIVATE_AUTH@node.test:443?obfs=salamander&obfs-password=PRIVATE_OBFS", "hy2", false),
            ("tuic://00000000-0000-4000-8000-000000000001:PRIVATE_AUTH@node.test:443", "tuic", false),
            ("vless://00000000-0000-4000-8000-000000000001@node.test:443?security=reality&pbk=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", "reality", false),
        ] {
            for engine in [crossbow_core::Engine::Mihomo,crossbow_core::Engine::SingBox] {
                let mut data=crossbow_core::StoreSchema::default();data.engine.engine=engine;
                let report=inspect_import(&data,uri,"test baseline".into());
                assert_eq!(report.status,if engine==crossbow_core::Engine::Mihomo||singbox {"compatible"}else{"unsupported"});
                assert_eq!(report.node_count,1);assert_eq!(report.protocols,[protocol]);
                let serialized=serde_json::to_string(&report).unwrap();
                for secret in ["PRIVATE_AUTH","PRIVATE_OBFS","node.test","hy2://"] {assert!(!serialized.contains(secret));}
            }
        }
    }
    #[test]
    fn malformed_mixed_and_remote_inputs_are_inspection_only() {
        let data = crossbow_core::StoreSchema::default();
        assert_eq!(
            inspect_import(&data, "https://subscription.test/token", String::new()).status,
            "remote"
        );
        assert_eq!(
            inspect_import(&data, "hy2://secret@host:443\nbad", String::new()).status,
            "invalid"
        );
        assert_eq!(
            inspect_import(&data, "https://host\nhy2://secret@host", String::new()).status,
            "invalid"
        );
        assert_eq!(inspect_import(&data, "", String::new()).status, "empty");
    }
}
