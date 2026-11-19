//! SSID 网络场景的查询与设置（#566）。
//!
//! 规则表持久化在 store（scenes 段）；变更后立即评估一次，让「把当前
//! 网络设为直连」这类操作无需等待下一次网络事件。

use std::collections::BTreeMap;

use tauri::{AppHandle, State};

use crate::app_state::AppState;
use crate::network::{scenes, ssid};

/// 设置页状态：规则表 + 当前网络 + 当前生效场景。
#[derive(serde::Serialize)]
pub(crate) struct SceneStatus {
    pub rules: BTreeMap<String, String>,
    /// 当前关联的 Wi-Fi；None = 无 Wi-Fi 或定位权限缺失（设置页据此提示）。
    pub current_ssid: Option<String>,
    /// 当前生效中的场景（SSID, 动作）；None = 未在任何场景中。
    pub active_scene: Option<(String, String)>,
}

#[tauri::command]
pub(crate) fn network_scenes_get(state: State<'_, AppState>) -> SceneStatus {
    let rules = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .scenes
        .rules
        .clone();
    SceneStatus {
        rules,
        current_ssid: ssid::current_ssid(),
        active_scene: scenes::active_scene(),
    }
}

/// 设置或更新一条场景规则；`action` 只接受 "direct" / "proxy"。
#[tauri::command]
pub(crate) fn network_scene_set(
    app: AppHandle,
    state: State<'_, AppState>,
    ssid: String,
    action: String,
) -> Result<(), String> {
    if !matches!(
        action.as_str(),
        scenes::ACTION_DIRECT | scenes::ACTION_PROXY
    ) {
        return Err(format!("未知场景动作：{action}"));
    }
    let trimmed = ssid.trim().to_string();
    if trimmed.is_empty() || trimmed.len() > 64 {
        return Err("网络名称不能为空且长度需在 64 字符内".into());
    }
    {
        let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store.data_mut().scenes.rules.insert(trimmed, action);
        store.save().map_err(|e| e.to_string())?;
    }
    scenes::evaluate_and_apply(&app);
    Ok(())
}

#[tauri::command]
pub(crate) fn network_scene_remove(
    app: AppHandle,
    state: State<'_, AppState>,
    ssid: String,
) -> Result<(), String> {
    {
        let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store.data_mut().scenes.rules.remove(&ssid);
        store.save().map_err(|e| e.to_string())?;
    }
    // 删除的可能正是当前生效场景 → 立即评估（离开恢复）。
    scenes::evaluate_and_apply(&app);
    Ok(())
}
