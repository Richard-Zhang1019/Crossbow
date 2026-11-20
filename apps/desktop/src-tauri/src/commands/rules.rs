//! 按进程分流规则的查询与增删改（#570）。
//!
//! 规则变更一律走 `commit_config_change`（验证 + 渲染 + 内核热重载），
//! 与覆写编辑同链路；失效目标（组名消失）由渲染层防御性丢弃。

use serde::Serialize;

// 自动生成自 lib.rs 的 command 分域提取（#423）惯例：use crate::* 引入根模块项。
use crate::*;
use tauri::State;

use crate::engine::flow::commit_config_change;
use crossbow_core::ProcessRule;

#[derive(Serialize)]
pub(crate) struct ProcessRuleStatus {
    pub rules: Vec<ProcessRule>,
    /// 当前订阅可用的策略目标（DIRECT/REJECT + proxy-groups 名），
    /// 供编辑器下拉选择。
    pub targets: Vec<String>,
}

/// Resolve policy names from the effective configuration, including overrides
/// and native sing-box outbound tags. A stale raw subscription is insufficient.
fn group_targets(state: &crate::AppState) -> Vec<String> {
    let content = {
        let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        crossbow_core::render_config(store.data())
            .map(|r| r.config)
            .unwrap_or_default()
    };
    let Ok(document) = serde_yaml::from_str::<serde_yaml::Value>(&content) else {
        return Vec::new();
    };
    policy_names(&document)
}

fn policy_names(document: &serde_yaml::Value) -> Vec<String> {
    let mut names = Vec::new();
    for (section, key) in [("proxy-groups", "name"), ("outbounds", "tag")] {
        if let Some(items) = document.get(section).and_then(|v| v.as_sequence()) {
            for item in items {
                if let Some(name) = item.get(key).and_then(|v| v.as_str()) {
                    if !matches!(name, "DIRECT" | "REJECT") && !names.iter().any(|n| n == name) {
                        names.push(name.to_owned());
                    }
                }
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    #[test]
    fn policy_choices_cover_native_singbox_and_mihomo_without_duplicate_builtins() {
        let yaml = serde_yaml::from_str("proxy-groups: [{name: auto}, {name: auto}]\n").unwrap();
        assert_eq!(super::policy_names(&yaml), vec!["auto"]);
        let native = serde_yaml::from_str(
            r#"{"outbounds":[{"tag":"DIRECT"},{"tag":"REJECT"},{"tag":"work"}]}"#,
        )
        .unwrap();
        assert_eq!(super::policy_names(&native), vec!["work"]);
    }
}

#[tauri::command]
pub(crate) fn process_rules_get(state: State<'_, AppState>) -> ProcessRuleStatus {
    let mut names = vec!["DIRECT".to_string(), "REJECT".to_string()];
    names.extend(group_targets(&state));
    let rules = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .process_rules
        .clone();
    ProcessRuleStatus {
        rules,
        targets: names,
    }
}

/// 新增或更新一条规则：`id` 为空即新增（生成 id），否则更新匹配项。
#[tauri::command]
pub(crate) fn process_rule_set(
    state: State<'_, AppState>,
    id: Option<String>,
    process: String,
    target: String,
) -> Result<(), String> {
    let process = process.trim().to_string();
    let target = target.trim().to_string();
    if !crossbow_core::process_rules::valid_process_name(&process) {
        return Err("进程名需为 1–128 个字符，不能包含逗号或控制字符".into());
    }
    if target.is_empty() || target.chars().any(|c| c == ',' || c.is_control()) {
        return Err("策略目标不能为空".into());
    }
    commit_config_change(&state, |data| {
        // Validate against the same effective render used by the transaction;
        // the active subscription may have changed while the editor was open.
        if !matches!(target.as_str(), "DIRECT" | "REJECT") {
            let rendered = crossbow_core::render_config(data).map_err(|e| e.to_string())?;
            let document: serde_yaml::Value =
                serde_yaml::from_str(&rendered.config).map_err(|e| e.to_string())?;
            let groups = document.get("proxy-groups").and_then(|v| v.as_sequence());
            let outbounds = document.get("outbounds").and_then(|v| v.as_sequence());
            let present = groups
                .into_iter()
                .flatten()
                .any(|g| g.get("name").and_then(|v| v.as_str()) == Some(target.as_str()))
                || outbounds
                    .into_iter()
                    .flatten()
                    .any(|g| g.get("tag").and_then(|v| v.as_str()) == Some(target.as_str()));
            if !present {
                return Err("所选策略目标已失效，请刷新后重新选择".into());
            }
        }
        match id.as_deref().unwrap_or_default() {
            "" => data.process_rules.push(ProcessRule {
                id: gen_id(),
                process,
                target,
            }),
            existing => data
                .process_rules
                .iter_mut()
                .find(|r| r.id == existing)
                .ok_or_else(|| "rule not found".to_string())
                .map(|rule| {
                    rule.process = process;
                    rule.target = target;
                })?,
        }
        Ok(())
    })
}

#[tauri::command]
pub(crate) fn process_rule_remove(state: State<'_, AppState>, id: String) -> Result<(), String> {
    commit_config_change(&state, |data| {
        let before = data.process_rules.len();
        data.process_rules.retain(|r| r.id != id);
        if data.process_rules.len() == before {
            return Err("rule not found".to_string());
        }
        Ok(())
    })
}
