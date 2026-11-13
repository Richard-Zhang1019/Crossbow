//! User-facing, credential-free maintenance metadata and bounded export commands.
use crate::AppState;
use crossbow_core::NodeChanges;
#[cfg(test)]
use crossbow_core::Store;
use serde::Serialize;
use std::collections::BTreeMap;
use tauri::{AppHandle, Emitter, Manager, State};

pub fn node_changes(before: &str, after: &str) -> Option<NodeChanges> {
    fn nodes(raw: &str) -> Option<BTreeMap<String, serde_yaml::Value>> {
        let value: serde_yaml::Value = serde_yaml::from_str(raw).ok()?;
        let mut map = BTreeMap::new();
        for node in value.get("proxies").and_then(|v| v.as_sequence())? {
            let name = node.get("name")?.as_str()?.to_owned();
            // Ambiguous duplicate names cannot support an honest count.
            if map.insert(name, node.clone()).is_some() {
                return None;
            }
        }
        Some(map)
    }
    let (old, new) = (nodes(before)?, nodes(after)?);
    Some(NodeChanges {
        added: new.keys().filter(|k| !old.contains_key(*k)).count(),
        removed: old.keys().filter(|k| !new.contains_key(*k)).count(),
        changed: new
            .iter()
            .filter(|(k, v)| old.get(*k).is_some_and(|old| old != *v))
            .count(),
    })
}
#[derive(Serialize)]
pub struct ProfileStatus {
    id: String,
    last_success: Option<u64>,
    next_update: Option<u64>,
    retry: bool,
    failure: Option<String>,
    failure_at: Option<u64>,
    using_previous: bool,
    changes: Option<NodeChanges>,
}
#[tauri::command]
pub fn profile_statuses(state: State<AppState>) -> Vec<ProfileStatus> {
    let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
    store
        .data()
        .profiles
        .iter()
        .map(|p| {
            let failure = p.last_update.as_ref().filter(|v| !v.ok).map(|v| {
                crate::notifications::redact(v.detail.as_deref().unwrap_or("Update failed"))
            });
            let retry_at = if p.update_interval_min > 0 {
                crate::scheduler::retry_at(&p.id)
            } else {
                None
            };
            ProfileStatus {
                id: p.id.clone(),
                last_success: p.last_updated,
                next_update: if p.update_interval_min == 0 {
                    None
                } else {
                    retry_at.or_else(|| {
                        p.last_updated
                            .map(|v| v.saturating_add(p.update_interval_min as u64 * 60))
                    })
                },
                retry: retry_at.is_some(),
                using_previous: failure.is_some(),
                failure_at: p.last_update.as_ref().filter(|v| !v.ok).map(|v| v.at),
                failure,
                changes: p
                    .last_update
                    .as_ref()
                    .filter(|v| v.ok)
                    .and_then(|v| v.changes.clone()),
            }
        })
        .collect()
}
#[derive(Serialize)]
pub struct SnapshotInfo {
    id: String,
    at: u64,
    profiles: usize,
    overrides: usize,
    active: Option<String>,
}
#[tauri::command]
pub fn config_snapshots(state: State<AppState>) -> Result<Vec<SnapshotInfo>, String> {
    let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
    let mut rows = Vec::new();
    for path in store
        .snapshots()
        .map_err(|e| e.to_string())?
        .into_iter()
        .rev()
    {
        let Some(id) = path.file_name().and_then(|v| v.to_str()) else {
            continue;
        };
        let Ok(data) = store.snapshot_data(id) else {
            continue;
        };
        let at = path
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|v| v.as_secs())
            .unwrap_or(0);
        rows.push(SnapshotInfo {
            id: id.into(),
            at,
            profiles: data.profiles.len(),
            overrides: data.overrides.len(),
            active: data
                .active_profile
                .as_ref()
                .and_then(|id| data.profile(id))
                .map(|p| p.name.clone()),
        });
    }
    Ok(rows)
}
fn recover_configuration(data: &mut crossbow_core::StoreSchema, saved: crossbow_core::StoreSchema) {
    data.profiles = saved.profiles;
    data.overrides = saved.overrides;
    data.active_profile = saved.active_profile;
}
#[tauri::command]
pub async fn restore_config_snapshot(app: AppHandle, id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let saved = state
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .snapshot_data(&id)
            .map_err(|e| e.to_string())?;
        // Configuration-only recovery: leave theme, ports, engine and takeover untouched.
        // commit_config_change validates, applies and rolls back runtime/disk failures.
        crate::commit_config_change(&state, move |data| {
            recover_configuration(data, saved);
            Ok(())
        })?;
        crate::scheduler::clear_all_retries();
        let _ = app.emit("profile://updated", "");
        crate::refresh_tray(&app);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}
fn clean_lines(lines: Vec<String>) -> Result<String, String> {
    if lines.len() > 2000 || lines.iter().map(String::len).sum::<usize>() > 4 * 1024 * 1024 {
        return Err("Log selection is too large".into());
    }
    Ok(lines
        .into_iter()
        .map(|s| redact_connection_line(&crate::notifications::redact(&s)))
        .collect::<Vec<_>>()
        .join("\n"))
}

/// #470：mihomo/sing-box 连接行的访问目标与出站节点名脱敏——通用 URL
/// 规则覆盖不到 `--> target:port using Node` 这类内核连接行格式。
/// 时间、级别与规则类别保留（定位所需），正则每次编译一次。
fn redact_connection_line(line: &str) -> String {
    static PATTERNS: std::sync::OnceLock<(regex::Regex, regex::Regex)> = std::sync::OnceLock::new();
    let (target, outbound) = PATTERNS.get_or_init(|| {
        (
            // [TCP] src --> target:port ... / --> target
            regex::Regex::new(r"(--> )\S+").unwrap(),
            // "... using Node-A" 出站节点链
            regex::Regex::new(r"(using )\S.*").unwrap(),
        )
    });
    let has_connection_shape = line.contains("-->") && line.contains("using ");
    let line = target.replace_all(line, "${1}[target redacted]");
    if line.contains("using ") && !line.contains("match DIRECT") && !line.contains("match GeoIP") {
        return outbound
            .replace_all(&line, "${1}[outbound redacted]")
            .to_string();
    }
    line.to_string()
}
#[tauri::command]
pub fn redact_log_lines(lines: Vec<String>) -> Result<String, String> {
    clean_lines(lines)
}
#[tauri::command]
pub async fn export_log_lines(lines: Vec<String>) -> Result<bool, String> {
    let clean = clean_lines(lines)?;
    let Some(file) = rfd::AsyncFileDialog::new()
        .set_file_name("crossbow-logs-redacted.txt")
        .add_filter("Text", &["txt"])
        .save_file()
        .await
    else {
        return Ok(false);
    };
    file.write(clean.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    Ok(true)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changes_count_identity_and_content_without_exporting_credentials() {
        let before = "proxies:\n- {name: a, server: old, password: secret}\n- {name: b}";
        let after = "proxies:\n- {name: a, server: new, password: other}\n- {name: c}";
        let c = node_changes(before, after).unwrap();
        assert_eq!((c.added, c.removed, c.changed), (1, 1, 1));
        assert!(node_changes("proxies: [{name: a}, {name: a}]", after).is_none());
        assert!(node_changes("{}", after).is_none());
    }
    #[test]
    fn recovery_keeps_engine_ui_and_takeover_preferences() {
        let mut current = crossbow_core::StoreSchema::default();
        current.engine.mixed_port = 7898;
        current.engine.tun_enable = true;
        current.ui.theme = "dark".into();
        let mut saved = crossbow_core::StoreSchema::default();
        saved.engine.mixed_port = 9000;
        saved.active_profile = Some("restored".into());
        saved.profiles.push(
            serde_json::from_str(r#"{"id":"restored","name":"Snapshot","kind":"local"}"#).unwrap(),
        );
        recover_configuration(&mut current, saved);
        assert_eq!(current.active_profile.as_deref(), Some("restored"));
        assert_eq!(current.profiles.len(), 1);
        assert_eq!(current.engine.mixed_port, 7898);
        assert!(current.engine.tun_enable);
        assert_eq!(current.ui.theme, "dark");
    }
    #[test]
    fn exports_are_bounded_and_redacted() {
        let s = clean_lines(vec![
            "https://user:pass@example.com/path?token=abc password=xyz Bearer qwerty".into(),
        ])
        .unwrap();
        for secret in ["user:pass", "/path", "abc", "xyz", "qwerty"] {
            assert!(!s.contains(secret));
        }
        assert!(clean_lines(vec![String::new(); 2001]).is_err());
    }
    #[test]
    fn snapshot_ids_cannot_escape_the_known_snapshot_list() {
        let dir = std::env::temp_dir().join(format!("crossbow-snapshot-{}", uuid::Uuid::new_v4()));
        let mut store = Store::open(&dir).unwrap();
        store.save().unwrap();
        store.save().unwrap();
        assert!(store.snapshot_data("../store.json").is_err());
        let id = store.snapshots().unwrap()[0]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert!(store.snapshot_data(&id).is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod redact_470_tests {
    use super::*;

    /// #470：mihomo 连接行的访问目标与出站节点名脱敏；时间/级别/规则类别保留。
    #[test]
    fn clean_lines_redacts_connection_targets_and_outbound() {
        let lines = vec![
            "[TCP] 127.0.0.1:53100 --> target.example.test:443 using Node-A".into(),
            "[TCP] 127.0.0.1:53101 --> private.internal.lan:80 match DomainSuffix(qq.com) using DIRECT".into(),
        ];
        let out = clean_lines(lines.clone()).unwrap();
        assert!(!out.contains("target.example.test"), "{out}");
        assert!(!out.contains("private.internal.lan"), "{out}");
        assert!(!out.contains("Node-A"), "{out}");
        assert!(out.contains("[target redacted]"), "{out}");
        // 时间戳与级别信息保留
        assert!(lines[0].starts_with("[TCP]"));
    }

    /// #470：未列协议 URI（hysteria2）的 userinfo 凭据脱敏。
    #[test]
    fn clean_lines_redacts_uncovered_scheme_userinfo() {
        let lines = vec!["hysteria2://fixture-password@node.example.test:443".into()];
        let out = clean_lines(lines).unwrap();
        assert!(!out.contains("fixture-password"), "{out}");
        assert!(out.contains("hysteria2://"), "{out}");
    }

    /// 保留既有能力：HTTPS 订阅路径/查询仍被移除（回归 #470 前）。
    #[test]
    fn clean_lines_keeps_existing_subscription_redaction() {
        let lines = vec!["https://sub.example.test/token?secret=abc".into()];
        let out = clean_lines(lines).unwrap();
        assert!(!out.contains("token?secret=abc"), "{out}");
        assert!(out.contains("sub.example.test"), "{out}");
    }

    /// DIRECT/GeoIP 行：目标保留（国内直连目标非敏感，且分类可定位）。
    #[test]
    fn clean_lines_keeps_direct_targets() {
        let lines = vec![
            "[TCP] 127.0.0.1:53101 --> qq.com:443 match DomainSuffix(qq.com) using DIRECT".into(),
        ];
        let out = clean_lines(lines).unwrap();
        assert!(out.contains("qq.com"), "{out}");
    }
}
