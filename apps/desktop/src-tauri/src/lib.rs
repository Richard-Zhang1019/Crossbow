//! Crossbow 桌面壳层（macOS + Windows，Tauri 2）。
//!
//! M0-S2 职责：Store 领域操作（导入/更新/删除/激活 Profile）+ 内核生命周期
//! （CoreManager）+ 状态事件推送。系统代理与托盘在 S3 加入。

use std::path::PathBuf;
use std::sync::Mutex;

use crossbow_core::model::TrafficInfo;
use crossbow_core::subscription::{fetch_subscription, now_secs, validate_profile_content};
use crossbow_core::{Profile, ProfileKind, Store};
use tauri::{Emitter, Manager, State};

mod core_manager;
use core_manager::{CoreManager, CoreStatus};

struct AppState {
    store: Mutex<Store>,
    core: CoreManager,
}

fn make_profile(id: String, name: String, kind: ProfileKind, url: Option<String>) -> Profile {
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
    }
}

fn gen_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// 导入 URL 订阅：拉取 → 校验 → 落库。
#[tauri::command]
fn import_profile_url(
    state: State<AppState>,
    url: String,
    name: Option<String>,
) -> Result<Profile, String> {
    let fetched = fetch_subscription(&url, 30).map_err(|e| e.to_string())?;
    validate_profile_content(&fetched.content).map_err(|e| e.to_string())?;
    let fallback = url.split('/').next_back().unwrap_or("订阅").to_string();
    let mut profile = make_profile(
        gen_id(),
        name.unwrap_or(fallback),
        ProfileKind::Remote,
        Some(url),
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

/// 导入本地/粘贴的配置内容。
#[tauri::command]
fn import_profile_content(
    state: State<AppState>,
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

/// 刷新订阅并落库；若该档案正在运行则热重启内核以应用新配置。
#[tauri::command]
fn update_profile(state: State<AppState>, id: String) -> Result<Profile, String> {
    let url = {
        let store = state.store.lock().unwrap();
        store
            .data()
            .profile(&id)
            .and_then(|p| p.url.clone())
            .ok_or("profile not found or has no url")?
    };
    let fetched = fetch_subscription(&url, 30).map_err(|e| e.to_string())?;
    validate_profile_content(&fetched.content).map_err(|e| e.to_string())?;

    let is_active_and_running = {
        let store = state.store.lock().unwrap();
        store.data().active_profile.as_deref() == Some(id.as_str())
            && state.core.status() == CoreStatus::Running
    };
    {
        let mut store = state.store.lock().unwrap();
        store
            .data_mut()
            .update_profile_content(id.as_str(), fetched.content, now_secs(), fetched.traffic)
            .map_err(|e| e.to_string())?;
        store.save().map_err(|e| e.to_string())?;
        let profile = store.data().profile(&id).cloned().unwrap();
        if is_active_and_running {
            start_core_locked(&state, &store)?;
        }
        Ok(profile)
    }
}

#[tauri::command]
fn set_active_profile(state: State<AppState>, id: String) -> Result<(), String> {
    let running = state.core.status() == CoreStatus::Running;
    {
        let mut store = state.store.lock().unwrap();
        store.data_mut().set_active_profile(&id)?;
        store.save().map_err(|e| e.to_string())?;
    }
    if running {
        let store = state.store.lock().unwrap();
        start_core_locked(&state, &store)?;
    }
    Ok(())
}

#[tauri::command]
fn remove_profile(state: State<AppState>, id: String) -> Result<(), String> {
    let mut store = state.store.lock().unwrap();
    store.data_mut().remove_profile(&id)?;
    store.save().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn list_profiles(state: State<AppState>) -> Vec<Profile> {
    state.store.lock().unwrap().data().profiles.clone()
}

#[tauri::command]
fn active_profile_id(state: State<AppState>) -> Option<String> {
    state.store.lock().unwrap().data().active_profile.clone()
}

/// 启动内核：渲染当前档案（含覆写）→ 注入运行时 → sidecar。
fn start_core_locked(state: &State<AppState>, store: &Store) -> Result<(), String> {
    let rendered = crossbow_core::render_config(store.data()).map_err(|e| e.to_string())?;
    let rt = crossbow_core::RuntimeConfig {
        mixed_port: store.data().engine.mixed_port,
        allow_lan: store.data().engine.allow_lan,
        ..crossbow_core::RuntimeConfig::default()
    };
    state.core.start(&rendered.config, &rt)
}

#[tauri::command]
fn core_start(state: State<AppState>) -> Result<(), String> {
    let store = state.store.lock().unwrap();
    start_core_locked(&state, &store)
}

#[tauri::command]
fn core_stop(state: State<AppState>) -> Result<(), String> {
    state.core.stop();
    Ok(())
}

#[tauri::command]
fn core_status(state: State<AppState>) -> CoreStatus {
    state.core.status()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let store = Store::open(&data_dir)
                .map_err(|e| std::io::Error::other(format!("open store: {e}")))?;

            // 内核二进制定位：环境变量优先（开发），其次应用目录 binaries/mihomo。
            let bin = std::env::var_os("CROSSBOW_MIHOMO_BIN")
                .map(PathBuf::from)
                .unwrap_or_else(|| data_dir.join("binaries").join("mihomo"));
            let core = CoreManager::new(bin, data_dir.join("runtime"));

            let handle = app.handle().clone();
            core.set_callback(std::sync::Arc::new(move |status| {
                let _ = handle.emit("core://status", status);
            }));

            app.manage(AppState {
                store: Mutex::new(store),
                core,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_profiles,
            active_profile_id,
            import_profile_url,
            import_profile_content,
            update_profile,
            set_active_profile,
            remove_profile,
            core_start,
            core_stop,
            core_status,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            // 任何退出路径都先收内核，避免残留进程。
            if let tauri::RunEvent::Exit = event {
                if let Some(state) = _app.try_state::<AppState>() {
                    state.core.stop();
                }
            }
        });
}

// TrafficInfo 引用占位：导出给后续命令复用（更新间隔设置等）。
#[allow(dead_code)]
type _Traffic = TrafficInfo;
