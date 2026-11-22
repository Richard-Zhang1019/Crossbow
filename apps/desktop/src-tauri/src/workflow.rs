//! User-facing, credential-free maintenance metadata and bounded export commands.
use crate::AppState;
use crossbow_core::config_package::{
    export_package, ConfigImportReport, ConfigPayload, MAX_PACKAGE_BYTES,
};
use crossbow_core::NodeChanges;
#[cfg(test)]
use crossbow_core::Store;
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::Read;
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
    data.process_rules = saved.process_rules;
    data.rule_sets = saved.rule_sets;
}
#[derive(Default)]
pub(crate) struct PackageSession {
    pending: Option<PendingPackage>,
    pub(crate) generation: u64,
}

struct PendingPackage {
    id: String,
    payload: ConfigPayload,
    target: [u8; 32],
}

#[derive(Serialize)]
pub struct PackagePreview {
    id: String,
    report: ConfigImportReport,
}

/// 预览依赖的规则集事实。缓存、更新时间和错误文本由后台刷新写入，
/// 若参与指纹，一次定时刷新就会让等待确认的预览误报「已过期」。
#[derive(serde::Serialize)]
struct RuleSetStamp<'a> {
    id: &'a str,
    name: &'a str,
    url: &'a str,
    behavior: crossbow_core::rule_sets::Behavior,
    target: &'a str,
    enabled: bool,
    interval_secs: u32,
}

fn rule_set_stamps(sets: &[crossbow_core::rule_sets::RuleSet]) -> Vec<RuleSetStamp<'_>> {
    sets.iter()
        .map(|set| RuleSetStamp {
            id: &set.id,
            name: &set.name,
            url: &set.url,
            behavior: set.behavior,
            target: &set.target,
            enabled: set.enabled,
            interval_secs: set.interval_secs,
        })
        .collect()
}

fn package_target(data: &crossbow_core::StoreSchema) -> Result<[u8; 32], String> {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    let rule_sets = rule_set_stamps(&data.rule_sets);
    serde_json::to_writer(
        &mut hash,
        &(
            &data.profiles,
            &data.overrides,
            &data.active_profile,
            &data.engine,
            &data.dns,
            &data.process_rules,
            &rule_sets,
        ),
    )
    .map_err(|_| "pkg.errorCommit".to_owned())?;
    Ok(hash.finalize().into())
}

fn read_config_package(path: &std::path::Path) -> Result<Vec<u8>, String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|_| "pkg.errorRead")?;
    let metadata = file.metadata().map_err(|_| "pkg.errorRead")?;
    if !metadata.is_file() {
        return Err("pkg.errorRead".into());
    }
    if metadata.len() > MAX_PACKAGE_BYTES as u64 {
        return Err("pkg.errorSize".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_PACKAGE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "pkg.errorRead")?;
    if bytes.len() > MAX_PACKAGE_BYTES {
        return Err("pkg.errorSize".into());
    }
    Ok(bytes)
}

#[tauri::command]
pub async fn export_config_package(app: AppHandle) -> Result<bool, String> {
    let Some(file) = rfd::AsyncFileDialog::new()
        .set_file_name("crossbow-config-package.json")
        .add_filter("JSON", &["json"])
        .save_file()
        .await
    else {
        return Ok(false);
    };
    let path = file.path().to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let session = state
            .package_session
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let data = state
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .data()
            .clone();
        drop(session);
        let text = export_package(&data).map_err(|e| e.to_string())?;
        crate::platform::fs_perms::write_private(&path, text.as_bytes())
            .map_err(|_| "pkg.errorWrite")?;
        Ok(true)
    })
    .await
    .map_err(|_| "pkg.errorWrite")?
}

#[tauri::command]
pub async fn preview_config_package(app: AppHandle) -> Result<Option<PackagePreview>, String> {
    let Some(file) = rfd::AsyncFileDialog::new()
        .add_filter("JSON", &["json"])
        .pick_file()
        .await
    else {
        return Ok(None);
    };
    let path = file.path().to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        let bytes = read_config_package(&path)?;
        let payload = ConfigPayload::parse(&bytes).map_err(|e| e.to_string())?;
        let state = app.state::<AppState>();
        let mut session = state
            .package_session
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        let report = payload.preview(store.data()).map_err(|e| e.to_string())?;
        let target = package_target(store.data())?;
        let id = uuid::Uuid::new_v4().to_string();
        session.pending = Some(PendingPackage {
            id: id.clone(),
            payload,
            target,
        });
        Ok(Some(PackagePreview { id, report }))
    })
    .await
    .map_err(|_| "pkg.errorRead")?
}

#[tauri::command]
pub fn cancel_config_package(state: State<AppState>, id: String) {
    let mut session = state
        .package_session
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if session.pending.as_ref().is_some_and(|p| p.id == id) {
        session.pending = None;
    }
}

#[tauri::command]
pub async fn import_config_package(app: AppHandle, id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let mut session = state
            .package_session
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let pending = session
            .pending
            .take_if(|p| p.id == id)
            .ok_or("pkg.errorStale")?;
        let mut validation_error = None;
        let mut active = None;
        // This lock also guards subscription refresh writes and their outcome
        // saves, so an old response cannot write over a just-restored profile.
        let result = crate::commit_config_change(&state, |data| {
            let changed = (|| -> Result<(), String> {
                if package_target(data)? != pending.target {
                    return Err("pkg.errorStale".into());
                }
                let changes_active = pending.payload.changes_active_profile(data);
                let policies = pending.payload.clone();
                pending
                    .payload
                    .merge_into(data)
                    .map_err(|e| e.to_string())?;
                if changes_active && data.active_profile.is_some() {
                    // The normal transaction skips engine apply when stopped.
                    // Validate the chosen profile and its enabled overrides anyway.
                    let rendered =
                        crossbow_core::render_config(data).map_err(|_| "pkg.errorActive")?;
                    policies
                        .validate_targets(&rendered.config)
                        .map_err(|e| e.to_string())?;
                    crate::platform::validate_config(data.engine.engine, &rendered.config)
                        .map_err(|_| "pkg.errorActive")?;
                }
                if data.active_profile.is_none() {
                    policies
                        .validate_targets("proxy-groups: []")
                        .map_err(|e| e.to_string())?;
                }
                active = data.active_profile.clone();
                Ok(())
            })();
            validation_error = changed.as_ref().err().cloned();
            changed
        });
        // Never forward runtime/YAML errors containing package contents to IPC.
        result.map_err(|_| validation_error.unwrap_or_else(|| "pkg.errorCommit".into()))?;
        session.generation = session.generation.wrapping_add(1);
        crate::scheduler::clear_all_retries();
        drop(session);
        let _ = app.emit("profile://updated", "");
        let _ = app.emit("profile://switched", active.as_deref().unwrap_or(""));
        crate::refresh_tray(&app);
        Ok(())
    })
    .await
    .map_err(|_| "pkg.errorCommit")?
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
        .iter()
        .map(|line| share_log_line(line))
        .collect::<Vec<_>>()
        .join("\n"))
}

/// Export only recognized categories. Free-form messages can contain arbitrary
/// credentials, user paths, node names and destinations even without URI syntax.
fn share_log_line(line: &str) -> String {
    let (level, message) = ["debug", "info", "warning", "error"]
        .into_iter()
        .find_map(|level| {
            line.strip_prefix('[')?
                .strip_prefix(level)?
                .strip_prefix("] ")
                .map(|message| (level, message))
        })
        .unwrap_or(("unknown", line));
    let protocol = ["TCP", "UDP", "TUN", "MIXED"].into_iter().find(|protocol| {
        message
            .strip_prefix('[')
            .and_then(|rest| rest.strip_prefix(protocol))
            .is_some_and(|rest| rest.starts_with("] "))
    });
    if let Some(protocol) = protocol {
        if let Some((_, right)) = message.split_once("-->") {
            let (target_and_rule, outbound) = right.rsplit_once(" using ").unwrap_or((right, ""));
            let rule = target_and_rule
                .split_once(" match ")
                .and_then(|(_, rule)| rule.split(['(', ' ', '\t']).next())
                .filter(|rule| {
                    matches!(
                        *rule,
                        "Match"
                            | "Domain"
                            | "DomainSuffix"
                            | "DomainKeyword"
                            | "GeoIP"
                            | "GeoSite"
                            | "IPCIDR"
                            | "IPCIDR6"
                            | "RuleSet"
                            | "ProcessName"
                            | "ProcessPath"
                            | "DstPort"
                            | "SrcPort"
                    )
                })
                .unwrap_or("unknown");
            let outbound = match outbound.trim() {
                "DIRECT" => "DIRECT",
                "REJECT" | "REJECT-DROP" => "REJECT",
                _ => "[outbound redacted]",
            };
            return format!(
                "[{level}] [{protocol}] [target redacted] match {rule} using {outbound}"
            );
        }
    }
    for (prefix, direction) in [
        ("inbound connection from ", "inbound"),
        ("inbound connection to ", "inbound"),
        ("outbound connection to ", "outbound"),
        ("inbound packet connection from ", "inbound"),
        ("outbound packet connection to ", "outbound"),
    ] {
        if message.contains(prefix) {
            return format!("[{level}] {direction} connection [endpoint redacted]");
        }
    }
    let normalized = message.to_ascii_lowercase();
    let category = [
        "timeout",
        "connection refused",
        "network unreachable",
        "dns",
        "tls",
        "authentication",
    ]
    .into_iter()
    .find(|category| normalized.contains(category))
    .unwrap_or("details omitted");
    format!("[{level}] [{category}]")
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
        saved.process_rules.push(crossbow_core::ProcessRule {
            id: "restored-rule".into(),
            process: "curl".into(),
            target: "DIRECT".into(),
        });
        saved.rule_sets.push(serde_json::from_value(serde_json::json!({"id":"restored-set","name":"Set","url":"https://example.invalid/rules","behavior":"domain","target":"DIRECT","enabled":true,"interval_secs":0,"payload":["example.invalid"],"updated_at":1})).unwrap());
        recover_configuration(&mut current, saved);
        assert_eq!(current.process_rules[0].id, "restored-rule");
        assert_eq!(current.rule_sets[0].id, "restored-set");
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
    fn package_preview_detects_changes_to_destination_configuration() {
        let mut target = crossbow_core::StoreSchema::default();
        let stamp = package_target(&target).unwrap();
        target.process_rules.push(crossbow_core::ProcessRule {
            id: "r".into(),
            process: "curl".into(),
            target: "DIRECT".into(),
        });
        assert_ne!(package_target(&target).unwrap(), stamp);
        target.process_rules.clear();
        target.dns.mode = crossbow_core::dns_settings::DnsMode::Custom;
        assert_ne!(package_target(&target).unwrap(), stamp);
        target.dns = crossbow_core::dns_settings::DnsSettings::default();
        target.ui.lang = "en".into();
        assert_eq!(package_target(&target).unwrap(), stamp);
        target.active_profile = Some("new-active".into());
        assert_ne!(package_target(&target).unwrap(), stamp);
        target.active_profile = None;
        target.engine.mixed_port += 1;
        assert_ne!(package_target(&target).unwrap(), stamp);
    }

    #[test]
    fn background_rule_set_refresh_does_not_invalidate_a_waiting_preview() {
        let mut target = crossbow_core::StoreSchema::default();
        target.rule_sets.push(
            serde_json::from_value(serde_json::json!({
                "id": "set", "name": "Set", "url": "https://example.invalid/rules",
                "behavior": "domain", "target": "DIRECT", "enabled": true,
                "interval_secs": 3600, "payload": ["a.example"], "updated_at": 1
            }))
            .unwrap(),
        );
        let stamp = package_target(&target).unwrap();
        // 定时刷新只改缓存/时间/错误，不改变导入结果，不能把待确认的预览判为过期。
        target.rule_sets[0].updated_at += 3600;
        target.rule_sets[0].last_error = Some("refresh failed".into());
        target.rule_sets[0].payload.push("b.example".into());
        assert_eq!(package_target(&target).unwrap(), stamp);
        // 启停与名称会改变注入结果和冲突计数，必须让预览失效。
        target.rule_sets[0].enabled = false;
        assert_ne!(package_target(&target).unwrap(), stamp);
        target.rule_sets[0].enabled = true;
        target.rule_sets[0].name = "Renamed".into();
        assert_ne!(package_target(&target).unwrap(), stamp);
    }

    #[test]
    fn package_reader_rejects_oversized_and_non_regular_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("package.json");
        std::fs::write(&path, b"{}").unwrap();
        assert_eq!(read_config_package(&path).unwrap(), b"{}");
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_PACKAGE_BYTES as u64 + 1)
            .unwrap();
        assert_eq!(read_config_package(&path).unwrap_err(), "pkg.errorSize");
        assert_eq!(
            read_config_package(dir.path()).unwrap_err(),
            "pkg.errorRead"
        );
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

    #[test]
    fn share_log_redacts_destinations_rule_values_and_credentials() {
        let fixtures = [
            "[info] [TCP] 127.0.0.1:53100 --> target.example.test:443 match GeoIP(private) using Node-A",
            "[info] [UDP] 127.0.0.1:53101 --> intranet.example:443 match DomainSuffix(intranet.example) using DIRECT",
            "[debug] [42] outbound/hysteria2[secret-node]: outbound connection to [2001:db8::22]:443",
            "[error] hysteria2://fixture-password@node.example:443 token: bare-secret /Users/fixture",
        ];
        let out = clean_lines(fixtures.map(str::to_owned).to_vec()).unwrap();
        assert_eq!(
            out,
            concat!(
                "[info] [TCP] [target redacted] match GeoIP using [outbound redacted]\n",
                "[info] [UDP] [target redacted] match DomainSuffix using DIRECT\n",
                "[debug] outbound connection [endpoint redacted]\n",
                "[error] [details omitted]",
            )
        );
    }
}
