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

    let mut store = state.store.lock().unwrap();
    store
        .data_mut()
        .add_profile(profile.clone())
        .map_err(|e| e.to_string())?;
    store.save().map_err(|e| e.to_string())?;
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
    validate_profile_content(&content).map_err(|e| e.to_string())?;
    let mut profile = make_profile(gen_id(), name, ProfileKind::Local, None);
    profile.content = content;
    profile.last_updated = Some(now_secs());

    let mut store = state.store.lock().unwrap();
    store
        .data_mut()
        .add_profile(profile.clone())
        .map_err(|e| e.to_string())?;
    store.save().map_err(|e| e.to_string())?;
    Ok(profile)
}

/// 刷新一次订阅（命令与调度器共用）：拉取 → 校验 → 当前档案走配置切换
/// （能热重载就热重载，失败退回旧内容）→ 记录结果。网络请求不持有 store 锁。
pub(crate) fn refresh_profile_blocking(
    state: &State<AppState>,
    id: &str,
) -> Result<Profile, String> {
    let url = {
        let store = state.store.lock().unwrap();
        store
            .data()
            .profile(id)
            .and_then(|p| p.url.clone())
            .ok_or("profile not found or has no url")?
    };
    let fetched = match fetch_subscription(&url, 30) {
        Ok(f) => f,
        Err(e) => {
            let _ = state.store.lock().unwrap().data_mut().set_profile_outcome(
                id,
                crossbow_core::UpdateOutcome {
                    ok: false,
                    changes: None,
                    detail: Some(e.to_string()),
                    at: now_secs(),
                },
            );
            let _ = state.store.lock().unwrap().save();
            return Err(e.to_string());
        }
    };
    if let Err(e) = validate_profile_content(&fetched.content) {
        let _ = state.store.lock().unwrap().data_mut().set_profile_outcome(
            id,
            crossbow_core::UpdateOutcome {
                ok: false,
                changes: None,
                detail: Some(e.to_string()),
                at: now_secs(),
            },
        );
        let _ = state.store.lock().unwrap().save();
        return Err(e.to_string());
    }

    let active = {
        let store = state.store.lock().unwrap();
        store.data().active_profile.as_deref() == Some(id)
    };
    let previous_content = state
        .store
        .lock()
        .unwrap()
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
        let mut store = state.store.lock().unwrap();
        store
            .data_mut()
            .update_profile_content(id, content, updated_at, traffic)?;
        store.save().map_err(|e| e.to_string())
    };
    if let Err(error) = apply {
        let _ = state.store.lock().unwrap().data_mut().set_profile_outcome(
            id,
            crossbow_core::UpdateOutcome {
                ok: false,
                changes: None,
                detail: Some(error.clone()),
                at: now_secs(),
            },
        );
        let _ = state.store.lock().unwrap().save();
        return Err(error);
    }
    let _ = state.store.lock().unwrap().data_mut().set_profile_outcome(
        id,
        crossbow_core::UpdateOutcome {
            ok: true,
            changes,
            detail: None,
            at: now_secs(),
        },
    );
    let _ = state.store.lock().unwrap().save();
    let profile = state
        .store
        .lock()
        .unwrap()
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
