// command 共享 helper（#423 Stage B 从 lib.rs 提取）。
use crate::*;
use tauri::State;
pub(crate) fn make_profile(
    id: String,
    name: String,
    kind: ProfileKind,
    url: Option<String>,
) -> Profile {
    Profile {
        id,
        name,
        kind,
        url,
        content: String::new(),
        update_interval_min: 0,
        override_ids: Vec::new(),
        last_updated: None,
        traffic: None,
        last_update: None,
    }
}

pub(crate) fn gen_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

// ---------- Profile 命令 ----------

/// 导入 URL 订阅的共享实现（命令与深链接共用）。
pub(crate) fn import_url_blocking(
    state: &AppState,
    url: &str,
    name: Option<&str>,
) -> Result<Profile, String> {
    // Share links pasted into the URL entry are local content, not HTTP requests.
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return import_content_blocking(
            state,
            name.unwrap_or("分享节点").to_owned(),
            url.to_owned(),
        );
    }
    let fetched = fetch_subscription(url, 30).map_err(|e| e.to_string())?;
    validate_profile_content(&fetched.content).map_err(|e| e.to_string())?;
    let fallback = url
        .split('/')
        .next_back()
        .unwrap_or("订阅")
        .split('?')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("订阅")
        .to_string();
    let mut profile = make_profile(
        gen_id(),
        name.unwrap_or(&fallback).to_string(),
        ProfileKind::Remote,
        Some(url.to_string()),
    );
    profile.content = fetched.content;
    profile.traffic = fetched.traffic;
    profile.last_updated = Some(now_secs());

    let _session = state
        .package_session
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
    persist_imported_profile(&mut store, &profile)?;
    Ok(profile)
}

/// 从文件名推断 Profile 名称（取去扩展名的文件名）。
pub(crate) fn profile_name_from_path(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "本地配置".into())
}

/// 本地内容导入的共享实现。
pub(crate) fn import_content_blocking(
    state: &AppState,
    name: String,
    content: String,
) -> Result<Profile, String> {
    let content = crossbow_core::subscription::normalize_profile_content(&content)
        .map_err(|e| e.to_string())?;
    let mut profile = make_profile(gen_id(), name, ProfileKind::Local, None);
    profile.content = content;
    profile.last_updated = Some(now_secs());

    let _session = state
        .package_session
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
    persist_imported_profile(&mut store, &profile)?;
    Ok(profile)
}

fn persist_imported_profile(store: &mut Store, profile: &Profile) -> Result<(), String> {
    store.transaction(|data| {
        validate_import_candidate(data, profile)?;
        data.add_profile(profile.clone())
    })
}

/// Validate against the engine inside the same Store transaction as publication.
/// Preview is advisory: import rechecks after any concurrent engine change.
pub(crate) fn validate_import_candidate(
    data: &crossbow_core::StoreSchema,
    profile: &Profile,
) -> Result<(), String> {
    let mut candidate = data.clone();
    candidate.profiles.push(profile.clone());
    candidate.active_profile = Some(profile.id.clone());
    crossbow_core::render_config(&candidate)
        .map(|_| ())
        .map_err(|_| {
            match data.engine.engine {
                crossbow_core::Engine::SingBox => {
                    "当前 sing-box 转换能力无法保留此配置；请使用 mihomo 或移除不支持的功能"
                }
                crossbow_core::Engine::Mihomo => {
                    "配置与当前 mihomo 设置不兼容，未导入；请检查配置和 DNS 设置"
                }
            }
            .to_string()
        })
}

pub(crate) const PROFILE_REFRESH_SUPERSEDED: &str =
    "The configuration changed while the subscription was updating";

/// 刷新一次订阅（命令与调度器共用）：拉取 → 校验 → 当前档案走配置切换
/// （能热重载就热重载，失败退回旧内容）→ 记录结果。网络请求不持有 store 锁。
pub(crate) fn refresh_profile_blocking(
    state: &State<AppState>,
    id: &str,
) -> Result<Profile, String> {
    let (generation, url) = {
        let session = state
            .package_session
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        let url = store
            .data()
            .profile(id)
            .and_then(|p| p.url.clone())
            .ok_or("profile not found or has no url")?;
        (session.generation, url)
    };
    let fetched = fetch_subscription(&url, 30);
    let session = state
        .package_session
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if session.generation != generation {
        return Err(PROFILE_REFRESH_SUPERSEDED.into());
    }
    let fetched = match fetched {
        Ok(f) => f,
        Err(e) => {
            let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
            let _ = store.transaction(|data| {
                data.set_profile_outcome(
                    id,
                    crossbow_core::UpdateOutcome {
                        ok: false,
                        changes: None,
                        detail: Some(e.to_string()),
                        at: now_secs(),
                    },
                )
            });
            return Err(e.to_string());
        }
    };
    if let Err(e) = validate_profile_content(&fetched.content) {
        let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        let _ = store.transaction(|data| {
            data.set_profile_outcome(
                id,
                crossbow_core::UpdateOutcome {
                    ok: false,
                    changes: None,
                    detail: Some(e.to_string()),
                    at: now_secs(),
                },
            )
        });
        return Err(e.to_string());
    }

    let active = {
        let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store.data().active_profile.as_deref() == Some(id)
    };
    let previous_content = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .profile(id)
        .map(|p| p.content.clone())
        .unwrap_or_default();
    let changes = workflow::node_changes(&previous_content, &fetched.content);
    let content = fetched.content.clone();
    let traffic = fetched.traffic;
    let updated_at = now_secs();
    // 当前档案才需要让内核跟上。其它档案只落盘，避免把正在用的网拆掉。
    let apply = if active {
        commit_config_change(state, |data| {
            data.update_profile_content(id, content.clone(), updated_at, traffic)
        })
    } else {
        let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store.transaction(|data| data.update_profile_content(id, content, updated_at, traffic))
    };
    if let Err(error) = apply {
        let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        let _ = store.transaction(|data| {
            data.set_profile_outcome(
                id,
                crossbow_core::UpdateOutcome {
                    ok: false,
                    changes: None,
                    detail: Some(error.clone()),
                    at: now_secs(),
                },
            )
        });
        return Err(error);
    }
    let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
    let _ = store.transaction(|data| {
        data.set_profile_outcome(
            id,
            crossbow_core::UpdateOutcome {
                ok: true,
                changes,
                detail: None,
                at: now_secs(),
            },
        )
    });

    let profile = store
        .data()
        .profile(id)
        .cloned()
        .ok_or("profile not found")?;
    Ok(profile)
}

/// 这条覆写是否挂在当前档案上。没挂上的修改不该碰正在跑的内核。
pub(crate) fn override_affects_active(state: &AppState, override_id: &str) -> bool {
    let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
    store
        .data()
        .active_profile
        .as_deref()
        .and_then(|pid| store.data().profile(pid))
        .is_some_and(|profile| profile.override_ids.iter().any(|id| id == override_id))
}

#[cfg(test)]
mod import_persistence_tests {
    use super::*;

    #[test]
    fn incompatible_import_preserves_active_profile_and_disk() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let mut old = make_profile("old".into(), "old".into(), ProfileKind::Local, None);
        old.content = "proxies: []\nrules: ['MATCH,DIRECT']".into();
        persist_imported_profile(&mut store, &old).unwrap();
        let mut unsupported = make_profile(
            "unsupported".into(),
            "unsupported".into(),
            ProfileKind::Local,
            None,
        );
        unsupported.content = crossbow_core::subscription::normalize_profile_content(
            "tuic://00000000-0000-4000-8000-000000000001:secret@host:443",
        )
        .unwrap();
        // A successful advisory preview cannot bypass a later engine change.
        validate_import_candidate(store.data(), &unsupported).unwrap();
        store
            .transaction(|data| {
                data.engine.engine = crossbow_core::Engine::SingBox;
                Ok(())
            })
            .unwrap();
        let before = std::fs::read(dir.path().join("store.json")).unwrap();
        let state = serde_json::to_value(store.data()).unwrap();
        assert!(persist_imported_profile(&mut store, &unsupported).is_err());
        assert_eq!(serde_json::to_value(store.data()).unwrap(), state);
        assert_eq!(
            std::fs::read(dir.path().join("store.json")).unwrap(),
            before
        );
    }
    #[test]
    fn remote_and_local_import_retry_only_publishes_one_profile() {
        for kind in [ProfileKind::Remote, ProfileKind::Local] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("store.json");
            let mut store = Store::open(dir.path()).unwrap();
            store.save().unwrap();
            let mut first = make_profile("failed".into(), "test".into(), kind, None);
            first.content = "proxies: []\nrules: ['MATCH,DIRECT']".into();
            let injection = crossbow_core::secure_write::inject_path_at(&path, 4);
            assert!(persist_imported_profile(&mut store, &first).is_err());
            drop(injection);
            assert!(store.data().profiles.is_empty());
            assert!(store.data().active_profile.is_none());
            let mut retry = make_profile("retry".into(), "test".into(), kind, None);
            retry.content = first.content.clone();
            persist_imported_profile(&mut store, &retry).unwrap();
            assert_eq!(store.data().profiles.len(), 1);
            assert_eq!(store.data().active_profile.as_deref(), Some("retry"));
            let reopened = Store::open(dir.path()).unwrap();
            assert_eq!(
                serde_json::to_value(reopened.data()).unwrap(),
                serde_json::to_value(store.data()).unwrap(),
            );
        }
    }
}
