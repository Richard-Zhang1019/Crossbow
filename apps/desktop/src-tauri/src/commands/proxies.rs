// 自动生成自 lib.rs 的 command 分域提取（#423）。use crate::* 引入根模块项。
use crate::engine::api::{Controller, GroupView, NodeDelayError, NODE_DELAY_REQUEST_BUDGET};
use crate::*;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

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

pub(crate) const NODE_DELAY_CONCURRENCY: usize = 10;

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct NodeDelayContext {
    revision: u64,
    pid: u32,
}

fn node_delay_context(state: &AppState) -> Option<NodeDelayContext> {
    let revision = state.node_delay_revision.load(Ordering::SeqCst);
    let pid = state.core().pid()?;
    (state.node_delay_session_current(Some(pid))
        && revision == state.node_delay_revision.load(Ordering::SeqCst))
    .then_some(NodeDelayContext { revision, pid })
}

#[derive(Clone)]
pub(crate) struct NodeDelayTask {
    id: String,
    context: NodeDelayContext,
    deadline: Instant,
    quality_scope: Option<crate::node_quality::Scope>,
    quality_epoch: u64,
    target: Option<crate::node_quality::Target>,
    nodes: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    node_limit: usize,
}

impl NodeDelayTask {
    fn admit(&self, node: &str) -> bool {
        let mut nodes = self.nodes.lock().unwrap_or_else(|e| e.into_inner());
        node.len() <= 4096 && nodes.len() < self.node_limit && nodes.insert(node.to_owned())
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NodeDelayTicket {
    id: String,
    budget_ms: u64,
}

fn node_delay_budget(node_count: usize) -> Duration {
    let batches = node_count.max(1).div_ceil(NODE_DELAY_CONCURRENCY) as u64;
    Duration::from_secs(
        batches
            .saturating_mul(NODE_DELAY_REQUEST_BUDGET.as_secs() * 2)
            .min(120),
    )
}

#[tauri::command]
pub(crate) fn begin_node_delay_test(
    state: State<AppState>,
    context: NodeDelayContext,
    node_count: usize,
    target: Option<crate::node_quality::Target>,
) -> Option<NodeDelayTicket> {
    let _operation = state.core_op.try_lock().ok()?;
    if node_delay_context(&state) != Some(context) {
        return None;
    }
    let engine = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .engine
        .engine;
    if engine == crossbow_core::Engine::SingBox
        && target.is_some_and(|t| t != crate::node_quality::Target::GoogleHttps)
    {
        return None;
    }
    let quality_scope = crate::node_quality::Scope::from_store(
        state.store.lock().unwrap_or_else(|e| e.into_inner()).data(),
    );
    let quality_epoch = state.node_quality.epoch();
    let mut task = state
        .node_delay_task
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if task
        .as_ref()
        .is_some_and(|task| task.context == context && Instant::now() < task.deadline)
    {
        return None;
    }
    let id = uuid::Uuid::new_v4().to_string();
    let budget = node_delay_budget(node_count);
    *task = Some(NodeDelayTask {
        id: id.clone(),
        context,
        deadline: Instant::now() + budget,
        quality_scope,
        quality_epoch,
        target,
        nodes: Default::default(),
        node_limit: node_count.clamp(1, 4096),
    });
    Some(NodeDelayTicket {
        id,
        budget_ms: budget.as_millis() as u64,
    })
}

#[tauri::command]
pub(crate) async fn end_node_delay_test(app: AppHandle, task_id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        {
            let mut task = state
                .node_delay_task
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if task.as_ref().is_some_and(|task| task.id == task_id) {
                *task = None;
            }
        }
        let result = state.node_quality.flush();
        if let Err(error) = &result {
            let mut notice = notifications::Notice::new(
                notifications::Kind::Error,
                "node-quality.save",
                "node-quality-storage",
            );
            notice.title_key = "proxies.quality.saveFailed".into();
            notice.details = error.clone();
            notifications::publish(&app, notice);
        }
        result
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn node_quality_report(
    app: AppHandle,
    node: String,
    context: NodeDelayContext,
) -> Result<crate::node_quality::Report, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        if node_delay_context(&state) != Some(context) {
            return Err("配置或内核已变化，请重新打开质量历史".into());
        }
        let scope = crate::node_quality::Scope::from_store(
            state.store.lock().unwrap_or_else(|e| e.into_inner()).data(),
        );
        let mut report = state.node_quality.report(scope.as_ref(), &node);
        if state
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .data()
            .engine
            .engine
            == crossbow_core::Engine::SingBox
        {
            report.test_targets = vec![crate::node_quality::Target::GoogleHttps];
        }
        if node_delay_context(&state) != Some(context) {
            return Err("配置或内核已变化，请重新打开质量历史".into());
        }
        Ok(report)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn node_quality_set_paused(app: AppHandle, paused: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<AppState>().node_quality.set_paused(paused)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn node_quality_clear(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || app.state::<AppState>().node_quality.clear())
        .await
        .map_err(|e| e.to_string())?
}

impl From<NodeDelayError> for NodeDelayOutcome {
    fn from(error: NodeDelayError) -> Self {
        let (class, detail) = match error {
            NodeDelayError::Timeout(detail) => ("timeout", detail),
            NodeDelayError::Controller(detail) => ("controller", detail),
            NodeDelayError::Failed(detail) => ("failed", detail),
            NodeDelayError::Cancelled(detail) => ("cancelled", detail),
            NodeDelayError::Skipped(detail) => ("skipped", detail),
        };
        Self {
            class: class.into(),
            delay_ms: None,
            reviewed: false,
            target: String::new(),
            tested_at_ms: 0,
            sample_count: 0,
            detail,
        }
    }
}

#[tauri::command]
pub(crate) async fn test_node_delay_structured(
    app: AppHandle,
    task_id: String,
    node: String,
) -> Result<NodeDelayOutcome, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let task = state
            .node_delay_task
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .filter(|task| task.id == task_id)
            .cloned();
        let cancelled = || NodeDelayError::Cancelled("测速已取消或配置已变化".into()).into();
        let Some(task) = task else {
            return Ok(cancelled());
        };
        if !task.admit(&node) {
            return Ok(NodeDelayError::Skipped("节点重复或批次请求数已达上限".into()).into());
        }
        let current = || {
            node_delay_context(&state) == Some(task.context)
                && state
                    .node_delay_task
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_ref()
                    .is_some_and(|active| active.id == task.id)
        };
        let core = state.core();
        let Some((port, secret)) = core.controller() else {
            return Ok(cancelled());
        };
        let controller = Controller { port, secret };
        let url_scoped_history = state
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .data()
            .engine
            .engine
            == crossbow_core::Engine::Mihomo;
        if !current() {
            return Ok(cancelled());
        }
        let Ok(_slot) = state.node_delay_slots.try_acquire() else {
            return Ok(NodeDelayError::Skipped("已有测速请求正在结束".into()).into());
        };
        Ok(
            match controller.measure_node_delay_observed(
                &node,
                &current,
                task.deadline,
                url_scoped_history,
                task.target,
                &|url, result| {
                    use crate::node_quality::{ResultClass, Sample, Target};
                    let Some(scope) = &task.quality_scope else {
                        return;
                    };
                    let Some(target) = Target::from_url(url) else {
                        return;
                    };
                    let class = match result {
                        Ok(_) => ResultClass::Ok,
                        Err(NodeDelayError::Timeout(_)) => ResultClass::Timeout,
                        Err(NodeDelayError::Failed(_)) => ResultClass::Failed,
                        Err(NodeDelayError::Controller(_)) => ResultClass::Controller,
                        _ => return,
                    };
                    // Cancellation fences publication and recording with the same task mutex.
                    let active = state
                        .node_delay_task
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if active.as_ref().is_some_and(|active| active.id == task.id)
                        && node_delay_context(&state) == Some(task.context)
                    {
                        state.node_quality.record(
                            task.quality_epoch,
                            scope,
                            &node,
                            target,
                            Sample {
                                at_ms: crate::node_quality::now_ms(),
                                class,
                                delay_ms: result.as_ref().ok().copied(),
                            },
                        );
                    }
                },
            ) {
                Ok(sample) => NodeDelayOutcome {
                    class: "ok".into(),
                    delay_ms: Some(sample.delay_ms),
                    reviewed: sample.reviewed,
                    target: sample.target,
                    tested_at_ms: sample.tested_at_ms,
                    sample_count: sample.sample_count,
                    detail: String::new(),
                },
                Err(error) => error.into(),
            },
        )
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
    read_proxies_snapshot(&state)
}

#[derive(serde::Serialize)]
pub(crate) struct ProxiesTestSnapshot {
    context: NodeDelayContext,
    groups: Vec<GroupView>,
}

#[tauri::command]
pub(crate) async fn proxies_test_snapshot(
    app: AppHandle,
) -> Result<Option<ProxiesTestSnapshot>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let Some(context) = node_delay_context(&state) else {
            return Ok(None);
        };
        let groups = read_proxies_snapshot(&state);
        if node_delay_context(&state) != Some(context) {
            return Ok(None);
        }
        Ok(Some(ProxiesTestSnapshot {
            context,
            groups: groups?,
        }))
    })
    .await
    .map_err(|e| e.to_string())?
}

fn read_proxies_snapshot(state: &AppState) -> Result<Vec<GroupView>, String> {
    let mut groups = controller_client(state)?.groups()?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_admission_is_shared_bounded_and_rejects_duplicate_nodes() {
        let task = NodeDelayTask {
            id: "test".into(),
            context: NodeDelayContext {
                revision: 0,
                pid: 1,
            },
            deadline: Instant::now(),
            quality_scope: None,
            quality_epoch: 0,
            target: None,
            nodes: Default::default(),
            node_limit: 2,
        };
        let other_worker = task.clone();
        assert!(!task.admit(&"x".repeat(4097)));
        assert!(task.admit("a"));
        assert!(!other_worker.admit("a"));
        assert!(other_worker.admit("b"));
        assert!(!task.admit("c"));
    }

    #[test]
    fn node_delay_batch_budget_accounts_for_both_probes_and_is_capped() {
        assert_eq!(node_delay_budget(1), 2 * NODE_DELAY_REQUEST_BUDGET);
        assert_eq!(node_delay_budget(10), node_delay_budget(1));
        assert_eq!(node_delay_budget(11), 2 * node_delay_budget(10));
        assert_eq!(node_delay_budget(usize::MAX), Duration::from_secs(120));
    }
}
