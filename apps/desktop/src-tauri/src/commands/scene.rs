//! SSID 网络场景的查询与设置（#566）。
//!
//! 规则表持久化在 store（scenes 段）；变更后立即评估一次，让「把当前
//! 网络设为直连」这类操作无需等待下一次网络事件。

use std::collections::BTreeMap;

use tauri::{AppHandle, Manager};

use crate::app_state::AppState;
use crate::network::{location, scenes, ssid};

/// 设置页状态：规则表 + 当前网络 + 当前生效场景 + 定位授权状态。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SceneStatus {
    pub supported: bool,
    pub network_state: String,
    pub rules: BTreeMap<String, String>,
    /// 当前关联的 Wi-Fi；None = 无 Wi-Fi 或定位权限缺失（设置页据此提示）。
    pub current_ssid: Option<String>,
    /// 当前生效中的场景（SSID, 动作）；None = 未在任何场景中。
    pub active_scene: Option<(String, String)>,
    /// 定位授权状态：not_determined 时前端展示「请求定位权限」入口，
    /// denied 时引导系统设置，authorized 而 SSID 为空才是真无 Wi-Fi。
    pub location: location::LocationPermission,
}

#[tauri::command]
pub(crate) async fn network_scenes_get(app: AppHandle) -> Result<SceneStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let rules = state
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .data()
            .scenes
            .rules
            .clone();
        let (current_ssid, network_state) = match ssid::observe() {
            ssid::Observation::Known(name) => (Some(name), "available"),
            ssid::Observation::Disconnected => (None, "disconnected"),
            ssid::Observation::Unavailable => (None, "unavailable"),
        };
        SceneStatus {
            rules,
            current_ssid,
            active_scene: scenes::active_scene(&state),
            supported: cfg!(target_os = "macos"),
            network_state: network_state.into(),
            location: location::status(),
        }
    })
    .await
    .map_err(|e| e.to_string())
}

/// 发起定位授权请求（首次弹系统授权框；允许后 Crossbow 出现在定位服务
/// 列表，SSID 探测才能拿到真实名称）。幂等：已授权/已拒绝时系统不再弹。
#[tauri::command]
pub(crate) async fn network_scene_request_location(
    app: AppHandle,
) -> Result<location::LocationPermission, String> {
    #[cfg(target_os = "macos")]
    {
        tauri::async_runtime::spawn_blocking(move || {
            let (sender, receiver) = std::sync::mpsc::channel();
            app.run_on_main_thread(move || {
                let _ = sender.send(location::request_when_in_use());
            })
            .map_err(|error| error.to_string())?;
            receiver
                .recv_timeout(std::time::Duration::from_secs(5))
                .map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| error.to_string())?
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Ok(location::status())
    }
}

#[tauri::command]
pub(crate) async fn network_scene_set(
    app: AppHandle,
    ssid: String,
    action: String,
) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("Network scenes require macOS".into());
    }
    if !matches!(
        action.as_str(),
        scenes::ACTION_DIRECT | scenes::ACTION_PROXY
    ) {
        return Err("Unknown scene action".into());
    }
    if ssid.trim().is_empty() || ssid.len() > 64 {
        return Err("Wi-Fi name must be 1–64 bytes".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        state.exit.ensure_open()?;
        {
            let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
            store.transaction(|data| {
                data.scenes.rules.insert(ssid, action);
                Ok(())
            })?;
        }
        scenes::evaluate_and_apply(&app);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn network_scene_remove(app: AppHandle, ssid: String) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("Network scenes require macOS".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        state.exit.ensure_open()?;
        {
            let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
            store.transaction(|data| {
                data.scenes.rules.remove(&ssid);
                Ok(())
            })?;
        }
        scenes::evaluate_and_apply(&app);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scene_status_wire_names_match_frontend() {
        let value = serde_json::to_value(SceneStatus {
            rules: BTreeMap::new(),
            current_ssid: Some("Home".into()),
            active_scene: Some(("Home".into(), "direct".into())),
            supported: true,
            network_state: "available".into(),
            location: crate::network::location::LocationPermission::NotDetermined,
        })
        .unwrap();
        assert_eq!(value["currentSsid"], "Home");
        assert_eq!(value["activeScene"][1], "direct");
        assert_eq!(value["networkState"], "available");
        assert_eq!(value["location"], "not_determined");
    }
}
