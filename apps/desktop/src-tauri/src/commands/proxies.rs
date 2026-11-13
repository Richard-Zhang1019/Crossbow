// 自动生成自 lib.rs 的 command 分域提取（#423）。use crate::* 引入根模块项。
use crate::*;
use tauri::{AppHandle, Emitter, Manager, State};

#[tauri::command]
pub(crate) async fn test_node_delay(
    state: State<'_, AppState>,
    node: String,
) -> Result<u64, String> {
    controller_client(&state)?.test_node_delay(&node)
}
#[tauri::command]
pub(crate) async fn test_group_delay(
    state: State<'_, AppState>,
    group: String,
) -> Result<BTreeMap<String, u64>, String> {
    controller_client(&state)?.test_group_delay(&group)
}
#[tauri::command]
pub(crate) async fn reset_proxy_selection(app: AppHandle, group: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let _op = state.proxy_op.lock().unwrap_or_else(|e| e.into_inner());
        controller_client(&state)?.reset_proxy_selection(&group)?;
        let _ = app.emit("proxy://selection-changed", ());
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) async fn select_proxy(
    app: AppHandle,
    group: String,
    name: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        select_proxy_inner(&app.state::<AppState>(), &group, &name)
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) fn proxies_snapshot(
    state: State<AppState>,
) -> Result<Vec<crate::engine::api::GroupView>, String> {
    let mut groups = controller_client(&state)?.groups()?;
    // 按订阅声明顺序展示（与 ClashX 一致，主选择组在前）；GLOBAL 恒最后
    let order = {
        let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store
            .data()
            .active_profile
            .clone()
            .and_then(|id| {
                store
                    .data()
                    .profile(&id)
                    .map(|p| crate::profile_group_order_str(&p.content))
            })
            .unwrap_or_default()
    };
    groups.sort_by(|a, b| {
        let ga = (a.name == "GLOBAL") as u8;
        let gb = (b.name == "GLOBAL") as u8;
        let pa = order
            .iter()
            .position(|n| n == &a.name)
            .unwrap_or(usize::MAX);
        let pb = order
            .iter()
            .position(|n| n == &b.name)
            .unwrap_or(usize::MAX);
        ga.cmp(&gb).then(pa.cmp(&pb))
    });
    Ok(groups)
}
