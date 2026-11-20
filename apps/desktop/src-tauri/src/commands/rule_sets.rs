//! Transactional rule-set edits. Network I/O is outside store/core locks.
use crate::engine::flow::commit_config_change;
use crate::*;
use crossbow_core::rule_sets::{self, Behavior, RuleSet};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Deserialize)]
pub(crate) struct Draft {
    id: Option<String>,
    name: String,
    url: String,
    behavior: Behavior,
    target: String,
    interval_secs: u32,
}
#[derive(Serialize)]
pub(crate) struct View {
    id: String,
    name: String,
    url: String,
    behavior: Behavior,
    target: String,
    enabled: bool,
    interval_secs: u32,
    updated_at: u64,
    last_error: Option<String>,
    count: usize,
}
#[derive(Serialize)]
pub(crate) struct Status {
    sets: Vec<View>,
    targets: Vec<String>,
}
fn policy_targets(data: &crossbow_core::StoreSchema, use_overrides: bool) -> Vec<String> {
    crossbow_core::render_config_with(data, use_overrides)
        .ok()
        .and_then(|v| serde_yaml::from_str::<serde_json::Value>(&v.config).ok())
        .map(|v| rule_sets::targets(&v))
        .unwrap_or_else(|| vec!["DIRECT".into(), "REJECT".into()])
}
#[tauri::command]
pub(crate) fn rule_sets_get(state: State<'_, AppState>) -> Status {
    let data = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .clone();
    let targets = policy_targets(
        &data,
        !state.safe_mode.load(std::sync::atomic::Ordering::SeqCst),
    );
    let sets = data
        .rule_sets
        .into_iter()
        .map(|s| View {
            count: s.payload.len(),
            id: s.id,
            name: s.name,
            url: s.url,
            behavior: s.behavior,
            target: s.target,
            enabled: s.enabled,
            interval_secs: s.interval_secs,
            updated_at: s.updated_at,
            last_error: s.last_error,
        })
        .collect();
    Status { sets, targets }
}
fn snapshot(state: &AppState, id: &str) -> Result<RuleSet, String> {
    state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .rule_sets
        .iter()
        .find(|s| s.id == id)
        .cloned()
        .ok_or("规则集不存在".into())
}
fn checked_target(
    data: &crossbow_core::StoreSchema,
    target: &str,
    use_overrides: bool,
) -> Result<(), String> {
    if target.is_empty()
        || target.contains(',')
        || target.chars().any(char::is_control)
        || !policy_targets(data, use_overrides)
            .iter()
            .any(|s| s == target)
    {
        return Err("策略目标已失效，请重新选择".into());
    }
    Ok(())
}
fn check_snapshot(data: &crossbow_core::StoreSchema, previous: &RuleSet) -> Result<(), String> {
    if data.rule_sets.iter().find(|s| s.id == previous.id) != Some(previous) {
        return Err("规则集已变化，已忽略旧请求；请重试".into());
    }
    Ok(())
}
fn notify_changed(state: &AppState) {
    let _ = state.app.emit("rulesets://updated", ());
}
fn save(state: &AppState, draft: Draft) -> Result<(), String> {
    let name = draft.name.trim().to_owned();
    let target = draft.target.trim().to_owned();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err("规则集名称需为 1–80 个字符".into());
    }
    if !(300..=604800).contains(&draft.interval_secs) {
        return Err("更新周期需为 300–604800 秒".into());
    }
    let url = rule_sets::canonical_url(&draft.url)?;
    let previous = draft
        .id
        .as_deref()
        .map(|id| snapshot(state, id))
        .transpose()?;
    let reused = previous
        .as_ref()
        .filter(|s| s.url == url && s.behavior == draft.behavior);
    let payload = if let Some(s) = reused {
        s.payload.clone()
    } else {
        rule_sets::fetch(&url, draft.behavior)?
    };
    let next = RuleSet {
        id: previous
            .as_ref()
            .map(|s| s.id.clone())
            .unwrap_or_else(gen_id),
        name,
        url,
        behavior: draft.behavior,
        target,
        enabled: previous.as_ref().is_none_or(|s| s.enabled),
        interval_secs: draft.interval_secs,
        payload,
        updated_at: reused.map(|s| s.updated_at).unwrap_or_else(now_secs),
        last_error: None,
    };
    let id = next.id.clone();
    commit_config_change(state, |data| {
        checked_target(
            data,
            &next.target,
            !state.safe_mode.load(std::sync::atomic::Ordering::SeqCst),
        )?;
        if let Some(old) = &previous {
            check_snapshot(data, old)?;
        }
        if data
            .rule_sets
            .iter()
            .any(|s| s.id != next.id && s.url == next.url)
        {
            return Err("该规则集 URL 已添加".into());
        }
        if previous.is_none() && data.rule_sets.len() >= rule_sets::MAX_SETS {
            return Err("最多支持 32 个规则集".into());
        }
        if let Some(slot) = data.rule_sets.iter_mut().find(|s| s.id == next.id) {
            *slot = next;
        } else {
            data.rule_sets.push(next);
        }
        if data.active_profile.is_some() {
            crossbow_core::render_config_with(
                data,
                !state.safe_mode.load(std::sync::atomic::Ordering::SeqCst),
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    })?;
    crate::scheduler::clear_retry(&format!("ruleset:{id}"));
    crate::notifications::resolve(&state.app, &format!("ruleset:{id}"));
    notify_changed(state);
    Ok(())
}
pub(crate) const SUPERSEDED: &str = "规则集已变化，已忽略旧请求；请重试";
fn update_metadata(
    state: &AppState,
    previous: &RuleSet,
    error: Option<String>,
    at: Option<u64>,
) -> Result<(), String> {
    let _op = state.core_op.lock().unwrap_or_else(|e| e.into_inner());
    state.exit.ensure_open()?;
    let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
    check_snapshot(store.data(), previous)?;
    let set = store
        .data_mut()
        .rule_sets
        .iter_mut()
        .find(|s| s.id == previous.id)
        .ok_or(SUPERSEDED)?;
    set.last_error = error;
    if let Some(at) = at {
        set.updated_at = at;
    }
    if let Err(error) = store.save() {
        if let Some(set) = store
            .data_mut()
            .rule_sets
            .iter_mut()
            .find(|s| s.id == previous.id)
        {
            *set = previous.clone();
        }
        return Err(error.to_string());
    }
    Ok(())
}
pub(crate) fn refresh(state: &AppState, id: &str) -> Result<(), String> {
    let previous = snapshot(state, id)?;
    let payload = match rule_sets::fetch(&previous.url, previous.behavior) {
        Ok(v) => v,
        Err(e) => {
            update_metadata(state, &previous, Some(e.clone()), None)?;
            notify_changed(state);
            return Err(e);
        }
    };
    let at = now_secs();
    if payload == previous.payload {
        update_metadata(state, &previous, None, Some(at))?;
    } else {
        let result = commit_config_change(state, |data| {
            check_snapshot(data, &previous)?;
            let set = data
                .rule_sets
                .iter_mut()
                .find(|s| s.id == id)
                .ok_or(SUPERSEDED)?;
            set.payload = payload;
            set.updated_at = at;
            set.last_error = None;
            if data.active_profile.is_some() {
                crossbow_core::render_config_with(
                    data,
                    !state.safe_mode.load(std::sync::atomic::Ordering::SeqCst),
                )
                .map_err(|e| e.to_string())?;
            }
            Ok(())
        });
        if let Err(error) = result {
            if error != SUPERSEDED {
                update_metadata(state, &previous, Some(error.clone()), None)?;
                notify_changed(state);
            }
            return Err(error);
        }
    }
    crate::scheduler::clear_retry(&format!("ruleset:{id}"));
    crate::notifications::resolve(&state.app, &format!("ruleset:{id}"));
    notify_changed(state);
    Ok(())
}
#[tauri::command]
pub(crate) async fn rule_set_save(app: AppHandle, draft: Draft) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || save(&app.state::<AppState>(), draft))
        .await
        .map_err(|_| "规则集任务异常结束".to_owned())?
}
#[tauri::command]
pub(crate) async fn rule_set_update(app: AppHandle, id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || refresh(&app.state::<AppState>(), &id))
        .await
        .map_err(|_| "规则集任务异常结束".to_owned())?
}
#[tauri::command]
pub(crate) async fn rule_set_change(
    app: AppHandle,
    id: String,
    action: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        commit_config_change(&state, |data| {
            let pos = data
                .rule_sets
                .iter()
                .position(|s| s.id == id)
                .ok_or("规则集不存在")?;
            match action.as_str() {
                "enable" => {
                    checked_target(
                        data,
                        &data.rule_sets[pos].target,
                        !state.safe_mode.load(std::sync::atomic::Ordering::SeqCst),
                    )?;
                    data.rule_sets[pos].enabled = true;
                }
                "disable" => data.rule_sets[pos].enabled = false,
                "remove" => {
                    data.rule_sets.remove(pos);
                }
                "up" if pos > 0 => data.rule_sets.swap(pos, pos - 1),
                "down" if pos + 1 < data.rule_sets.len() => data.rule_sets.swap(pos, pos + 1),
                "up" | "down" => {}
                _ => return Err("未知规则集操作".into()),
            }
            if data.active_profile.is_some() {
                crossbow_core::render_config_with(
                    data,
                    !state.safe_mode.load(std::sync::atomic::Ordering::SeqCst),
                )
                .map_err(|e| e.to_string())?;
            }
            Ok(())
        })?;
        crate::scheduler::clear_retry(&format!("ruleset:{id}"));
        crate::notifications::resolve(&state.app, &format!("ruleset:{id}"));
        notify_changed(&state);
        Ok(())
    })
    .await
    .map_err(|_| "规则集任务异常结束".to_owned())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_fetch_cannot_overwrite_edit_disable_remove_or_new_cache() {
        let old = RuleSet {
            id: "r".into(),
            name: "AI".into(),
            url: "https://example.invalid/list".into(),
            behavior: Behavior::Domain,
            target: "DIRECT".into(),
            enabled: true,
            interval_secs: 3600,
            payload: vec!["+.example.com".into()],
            updated_at: 1,
            last_error: None,
        };
        let mut data = crossbow_core::StoreSchema::default();
        data.rule_sets.push(old.clone());
        assert!(check_snapshot(&data, &old).is_ok());
        for next in [
            RuleSet {
                enabled: false,
                ..old.clone()
            },
            RuleSet {
                target: "REJECT".into(),
                ..old.clone()
            },
            RuleSet {
                updated_at: 2,
                ..old.clone()
            },
        ] {
            data.rule_sets = vec![next];
            assert!(check_snapshot(&data, &old).is_err());
        }
        data.rule_sets.clear();
        assert!(check_snapshot(&data, &old).is_err());
    }
}
