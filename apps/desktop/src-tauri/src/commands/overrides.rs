// 自动生成自 lib.rs 的 command 分域提取（#423）。use crate::* 引入根模块项。
use crate::*;
use tauri::{AppHandle, State};

/// 切换覆写启用态。
#[tauri::command]
pub(crate) async fn set_override_enabled(
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    let affects_running = override_affects_active(&state, &id);
    let edit = |data: &mut crossbow_core::StoreSchema| {
        let override_def = data
            .overrides
            .iter_mut()
            .find(|o| o.id == id)
            .ok_or("override not found")?;
        override_def.enabled = enabled;
        Ok(())
    };
    if affects_running {
        commit_config_change(&state, edit)?;
    } else {
        let mut store = state.store.lock().unwrap();
        edit(store.data_mut())?;
        store.save().map_err(|e| e.to_string())?;
    }
    Ok(())
}
/// 绑定/解绑覆写到 Profile。
#[tauri::command]
pub(crate) async fn toggle_override_binding(
    state: State<'_, AppState>,
    profile_id: String,
    override_id: String,
    enabled: bool,
) -> Result<(), String> {
    let affects_running = {
        let store = state.store.lock().unwrap();
        store.data().active_profile.as_deref() == Some(profile_id.as_str())
    };
    let edit = |data: &mut crossbow_core::StoreSchema| {
        let profile = data
            .profiles
            .iter_mut()
            .find(|p| p.id == profile_id)
            .ok_or("profile not found")?;
        if enabled {
            if !profile.override_ids.contains(&override_id) {
                profile.override_ids.push(override_id);
            }
        } else {
            profile.override_ids.retain(|oid| oid != &override_id);
        }
        Ok(())
    };
    if affects_running {
        commit_config_change(&state, edit)?;
    } else {
        let mut store = state.store.lock().unwrap();
        edit(store.data_mut())?;
        store.save().map_err(|e| e.to_string())?;
    }
    Ok(())
}
/// 删除覆写（并从所有 Profile 摘除引用）。
#[tauri::command]
pub(crate) async fn remove_override(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let affects_running = override_affects_active(&state, &id);
    let edit = |data: &mut crossbow_core::StoreSchema| {
        data.overrides.retain(|o| o.id != id);
        for profile in data.profiles.iter_mut() {
            profile.override_ids.retain(|oid| oid != &id);
        }
        Ok(())
    };
    if affects_running {
        commit_config_change(&state, edit)?;
    } else {
        let mut store = state.store.lock().unwrap();
        edit(store.data_mut())?;
        store.save().map_err(|e| e.to_string())?;
    }
    refresh_tray(&app);
    Ok(())
}
#[tauri::command]
pub(crate) fn update_override(
    state: State<AppState>,
    id: String,
    content: Option<String>,
    name: Option<String>,
) -> Result<(), String> {
    let affects_running = content.is_some() && override_affects_active(&state, &id);
    let edit = |data: &mut crossbow_core::StoreSchema| {
        let o = data
            .overrides
            .iter_mut()
            .find(|o| o.id == id)
            .ok_or("override not found")?;
        if let Some(c) = content {
            o.content = c;
        }
        if let Some(n) = name {
            o.name = n;
        }
        Ok(())
    };
    if affects_running {
        commit_config_change(&state, edit)?;
    } else {
        let mut store = state.store.lock().unwrap();
        edit(store.data_mut())?;
        store.save().map_err(|e| e.to_string())?;
    }
    Ok(())
}
/// 校验通过后应用覆写：复位安全模式，运行中则热重启。
#[tauri::command]
pub(crate) async fn apply_overrides(state: State<'_, AppState>) -> Result<(), String> {
    commit_config_change_inner(&state, |_| Ok(()), true)
}
/// 更新覆写内容/名称。
/// 校验当前覆写链（渲染一遍，错误记录到对应覆写）。
#[tauri::command]
pub(crate) async fn validate_overrides(state: State<'_, AppState>) -> Result<(), String> {
    let mut store = state.store.lock().unwrap();
    let failures = crossbow_core::validate_enabled_overrides(store.data());
    for (id, err) in &failures {
        store.data_mut().set_override_error(id, Some(err.clone()));
    }
    // 清除本次校验通过的启用覆写的错误；停用的保留原状
    for o in store.data_mut().overrides.iter_mut() {
        if o.enabled && !failures.iter().any(|(id, _)| id == &o.id) {
            o.last_error = None;
        }
    }
    store.save().map_err(|e| e.to_string())?;
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("{} 个覆写校验失败", failures.len()))
    }
}
#[tauri::command]
pub(crate) async fn create_override(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    kind: String,
) -> Result<crossbow_core::OverrideDef, String> {
    let kind = match kind.as_str() {
        "script" => crossbow_core::OverrideKind::Script,
        "merge" => crossbow_core::OverrideKind::Merge,
        _ => return Err(format!("unknown kind: {kind}")),
    };
    let def = crossbow_core::OverrideDef {
        id: gen_id(),
        name,
        kind,
        last_error: None,
        enabled: true,
        content: if kind == crossbow_core::OverrideKind::Script {
            "function main(config) {
  // 在此编辑覆写脚本
  return config;
}"
            .into()
        } else {
            "# merge 补丁示例：覆盖端口
# mixed-port: 7897
"
            .into()
        },
    };
    {
        let mut store = state.store.lock().unwrap();
        store.data_mut().overrides.push(def.clone());
        store.save().map_err(|e| e.to_string())?;
    }
    refresh_tray(&app);
    Ok(def)
}
/// 创建覆写（默认 Script 或 Merge）。
/// 列出全部覆写。
#[tauri::command]
pub(crate) async fn list_overrides(
    state: State<'_, AppState>,
) -> Result<Vec<crossbow_core::OverrideDef>, String> {
    Ok(state.store.lock().unwrap().data().overrides.clone())
}
