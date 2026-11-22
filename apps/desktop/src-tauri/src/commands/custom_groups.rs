use crate::*;
use crossbow_core::{
    custom_groups::{self, Group, Resolution},
    StoreSchema,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::State;
fn context(data: &StoreSchema, safe: bool) -> String {
    let value = serde_json::to_vec(&(
        &data.active_profile,
        &data.profiles,
        &data.overrides,
        &data.engine,
        &data.dns,
        safe,
    ))
    .unwrap_or_default();
    format!("{:x}", Sha256::digest(value))
}
fn source(data: &StoreSchema, safe: bool) -> Result<serde_json::Value, String> {
    let mut source = data.clone();
    source.custom_groups.clear();
    source.engine.flag_emoji = false;
    source.engine.engine = crossbow_core::Engine::Mihomo;
    let rendered = crossbow_core::render_config_with(&source, !safe).map_err(|_| {
        "当前来源无法解析或不是 Clash 内联节点配置 / Unsupported source configuration".to_string()
    })?;
    serde_yaml::from_str(&rendered.config).map_err(|_| "来源配置无效 / Invalid source".into())
}
#[derive(Serialize)]
pub(crate) struct Status {
    sources: Vec<(String, String)>,
    groups: Vec<Group>,
    profile_id: Option<String>,
    profile_name: Option<String>,
    candidates: Vec<String>,
    context: String,
    error: Option<String>,
    resolutions: Vec<(String, Resolution)>,
}
#[tauri::command]
pub(crate) fn custom_groups_get(state: State<'_, AppState>) -> Status {
    let data = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .clone();
    let safe = state.safe_mode.load(std::sync::atomic::Ordering::SeqCst);
    let doc = source(&data, safe);
    let candidates = doc
        .as_ref()
        .map_err(Clone::clone)
        .and_then(custom_groups::candidates);
    let resolutions = data
        .custom_groups
        .iter()
        .map(|g| {
            let result = if data.active_profile.as_deref() != Some(&g.profile_id) {
                Resolution {
                    members: vec![],
                    missing: vec![],
                    error: Some("来源未激活或已移除 / Source inactive or removed".into()),
                }
            } else {
                match &doc {
                    Ok(d) => custom_groups::resolve(d, g),
                    Err(e) => Resolution {
                        members: vec![],
                        missing: vec![],
                        error: Some(e.clone()),
                    },
                }
            };
            (g.id.clone(), result)
        })
        .collect();
    Status {
        sources: data
            .profiles
            .iter()
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect(),
        context: context(&data, safe),
        profile_name: data
            .active_profile
            .as_deref()
            .and_then(|id| data.profile(id))
            .map(|p| p.name.clone()),
        profile_id: data.active_profile.clone(),
        groups: data.custom_groups.clone(),
        error: candidates.as_ref().err().cloned(),
        candidates: candidates.unwrap_or_default(),
        resolutions,
    }
}
#[tauri::command]
pub(crate) fn preview_custom_group(
    state: State<'_, AppState>,
    draft: Group,
    source_context: String,
) -> Result<Resolution, String> {
    let data = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .clone();
    let safe = state.safe_mode.load(std::sync::atomic::Ordering::SeqCst);
    if context(&data, safe) != source_context
        || data.active_profile.as_deref() != Some(&draft.profile_id)
    {
        return Err("来源已变化，请刷新 / Source changed; reload".into());
    }
    let mut proposed = data.custom_groups.clone();
    proposed.retain(|g| g.id != draft.id);
    proposed.push(draft.clone());
    if let Err(error) = custom_groups::validate(&proposed) {
        return Ok(Resolution {
            members: vec![],
            missing: vec![],
            error: Some(error),
        });
    }
    let mut resolution = custom_groups::resolve(&source(&data, safe)?, &draft);
    if resolution.error.is_none() {
        let mut candidate = data.clone();
        candidate.custom_groups = proposed;
        match crossbow_core::render_config_with(&candidate, !safe) {
            Ok(rendered) => {
                let doc: serde_json::Value = serde_yaml::from_str(&rendered.config)
                    .map_err(|_| "Preview encoding failed".to_string())?;
                let group = doc["proxy-groups"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|g| g["name"] == draft.name)
                    .or_else(|| {
                        doc["outbounds"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .find(|g| g["tag"] == draft.name)
                    });
                if let Some(group) = group {
                    resolution.members = group
                        .get("proxies")
                        .or_else(|| group.get("outbounds"))
                        .and_then(|v| v.as_array())
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect();
                }
            }
            Err(_) => {
                resolution.error=Some("当前内核不能等价渲染此组或现有配置 / Current core cannot render this group or configuration equivalently".into());
                resolution.members.clear();
            }
        }
    }
    Ok(resolution)
}
#[tauri::command]
pub(crate) fn custom_groups_set(
    state: State<'_, AppState>,
    expected: Vec<Group>,
    groups: Vec<Group>,
    source_context: String,
) -> Result<(), String> {
    crate::engine::flow::commit_config_change(&state, |data| {
        let safe = state.safe_mode.load(std::sync::atomic::Ordering::SeqCst);
        apply(data, safe, &expected, groups, &source_context)
    })
}
// Called under the existing serialized config transaction; failures roll back store and runtime.
fn apply(
    data: &mut StoreSchema,
    safe: bool,
    expected: &[Group],
    groups: Vec<Group>,
    source_context: &str,
) -> Result<(), String> {
    custom_groups::validate(&groups)?;
    if data.custom_groups != expected || context(data, safe) != source_context {
        return Err("来源或组列表已变化，请刷新 / Source or groups changed; reload".into());
    }
    let document = source(data, safe).ok();
    for old in &data.custom_groups {
        if groups
            .iter()
            .any(|g| g.id == old.id && g.name == old.name && g.profile_id == old.profile_id)
        {
            continue;
        }
        if document.as_ref().is_some_and(|doc| {
            doc["proxy-groups"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|g| {
                    g["proxies"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|m| m == &old.name)
                })
                || doc["rules"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|r| r.as_str())
                    .any(|r| r.split(',').any(|part| part.trim() == old.name))
        }) {
            return Err("来源配置可能仍引用此组，不能删除或重命名 / Source configuration still references this group".into());
        }
        if data.process_rules.iter().any(|r| r.target == old.name)
            || data
                .user_rules
                .iter()
                .any(|r| r.enabled && r.target == old.name)
            || data
                .rule_sets
                .iter()
                .any(|r| r.enabled && r.target == old.name)
        {
            return Err(
                "组仍被分流规则引用，不能删除或重命名 / Group is referenced by routing rules"
                    .into(),
            );
        }
    }
    for g in &groups {
        if !expected.contains(g) && data.active_profile.as_deref() != Some(&g.profile_id) {
            return Err("请先激活该组来源 / Activate this group's source first".into());
        }
    }
    data.custom_groups = groups;
    if data.active_profile.is_some() {
        crossbow_core::render_config_with(data,!safe).map_err(|_|"自定义组无法应用，原配置保留 / Custom group could not be applied; existing configuration preserved".to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn data() -> StoreSchema {
        let mut data = StoreSchema::default();
        data.engine.flag_emoji = false;
        data.profiles.push(serde_json::from_value(json!({"id":"p","name":"Source","kind":"local","content":"proxies: [{name: A, type: http, server: localhost, port: 80}]\nproxy-groups: [{name: Existing, type: select, proxies: [A]}]\nrules: ['MATCH,Existing']"})).unwrap());
        data.active_profile = Some("p".into());
        data
    }
    fn group() -> Group {
        serde_json::from_value(json!({"id":"g","profile_id":"p","name":"Personal","kind":"select","membership":"explicit","members":["A"],"filter":"","url":"","interval":300,"tolerance":50})).unwrap()
    }
    #[test]
    fn stale_source_or_list_cannot_overwrite_new_state() {
        let mut data = data();
        let stamp = context(&data, false);
        data.profiles[0].content.push_str("\n# source updated");
        assert!(apply(&mut data, false, &[], vec![group()], &stamp).is_err());
        assert!(data.custom_groups.is_empty());
        let stamp = context(&data, false);
        data.custom_groups.push(group());
        assert!(apply(&mut data, false, &[], vec![], &stamp).is_err());
        assert_eq!(data.custom_groups, [group()]);
    }
    #[test]
    fn referenced_deletion_is_blocked_and_invalid_update_is_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = crossbow_core::Store::open(dir.path()).unwrap();
        *store.data_mut() = data();
        let stamp = context(store.data(), false);
        store
            .transaction(|d| apply(d, false, &[], vec![group()], &stamp))
            .unwrap();
        store
            .data_mut()
            .user_rules
            .push(crossbow_core::user_rules::UserRule {
                id: "route".into(),
                kind: crossbow_core::user_rules::Kind::Domain,
                value: "example.com".into(),
                target: "Personal".into(),
                enabled: true,
            });
        let stamp = context(store.data(), false);
        assert!(store
            .transaction(|d| apply(d, false, &[group()], vec![], &stamp))
            .is_err());
        let mut invalid = group();
        invalid.members = vec!["gone".into()];
        assert!(store
            .transaction(|d| apply(d, false, &[group()], vec![invalid], &stamp))
            .is_err());
        assert_eq!(store.data().custom_groups, [group()]);
        let reopened = crossbow_core::Store::open(dir.path()).unwrap();
        assert_eq!(reopened.data().custom_groups, [group()]);
    }
}
