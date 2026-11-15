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

/// #501：结构化单节点测速结果——错误按类别返回（含取消/跳过），复核
/// 样本带目标、时间与样本数；不把控制器故障混成节点超时。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NodeDelayOutcome {
    /// "ok" | "timeout" | "controller" | "failed" | "cancelled" | "skipped"
    pub class: String,
    pub delay_ms: Option<u64>,
    pub reviewed: bool,
    pub target: String,
    pub tested_at_ms: u64,
    pub sample_count: u8,
    pub detail: String,
}

#[tauri::command]
pub(crate) async fn test_node_delay_structured(
    app: AppHandle,
    node: String,
) -> Result<NodeDelayOutcome, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        // 复用托盘同款的会话/配置守卫与引擎感知目标策略（mihomo 复核，
        // sing-box 固定 HTTPS 主目标）。
        let core = state.core();
        let pid = core.pid();
        let (port, secret) = core.controller().ok_or_else(|| "内核未运行".to_string())?;
        let controller = crate::engine::api::Controller { port, secret };
        let url_scoped_history = {
            let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
            store.data().engine.engine == crossbow_core::Engine::Mihomo
        };
        let current = || state.node_delay_session_current(pid);
        // 主目标 5s + 复核 5s + 余量。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(12);
        match controller.measure_node_delay_with_review(
            &node,
            &current,
            deadline,
            url_scoped_history,
        ) {
            Ok(sample) => Ok(NodeDelayOutcome {
                class: "ok".into(),
                delay_ms: Some(sample.delay_ms),
                reviewed: sample.reviewed,
                target: sample.target,
                tested_at_ms: sample.tested_at_ms,
                sample_count: sample.sample_count,
                detail: String::new(),
            }),
            Err(crate::engine::api::NodeDelayError::Timeout(detail)) => Ok(NodeDelayOutcome {
                class: "timeout".into(),
                delay_ms: None,
                reviewed: false,
                target: String::new(),
                tested_at_ms: 0,
                sample_count: 0,
                detail,
            }),
            Err(crate::engine::api::NodeDelayError::Controller(detail)) => Ok(NodeDelayOutcome {
                class: "controller".into(),
                delay_ms: None,
                reviewed: false,
                target: String::new(),
                tested_at_ms: 0,
                sample_count: 0,
                detail,
            }),
            Err(crate::engine::api::NodeDelayError::Failed(detail)) => Ok(NodeDelayOutcome {
                class: "failed".into(),
                delay_ms: None,
                reviewed: false,
                target: String::new(),
                tested_at_ms: 0,
                sample_count: 0,
                detail,
            }),
            Err(crate::engine::api::NodeDelayError::Cancelled(detail)) => Ok(NodeDelayOutcome {
                class: "cancelled".into(),
                delay_ms: None,
                reviewed: false,
                target: String::new(),
                tested_at_ms: 0,
                sample_count: 0,
                detail,
            }),
            Err(crate::engine::api::NodeDelayError::Skipped(detail)) => Ok(NodeDelayOutcome {
                class: "skipped".into(),
                delay_ms: None,
                reviewed: false,
                target: String::new(),
                tested_at_ms: 0,
                sample_count: 0,
                detail,
            }),
        }
    })
    .await
    .map_err(|e| e.to_string())?
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
