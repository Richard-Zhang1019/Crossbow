//! Crossbow 桌面壳层（macOS + Windows，Tauri 2）。
//!
//! M0-S4 职责：在 S3（系统代理/托盘/关窗驻留）之上加入 WS 数据桥
//! （traffic/connections/logs 节流聚合）、设置（端口/局域网/主题/自启/
//! 内核版本）、深链接导入与单实例。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use crossbow_core::subscription::{fetch_subscription, now_secs, validate_profile_content};
use crossbow_core::{Profile, ProfileKind, RuntimeConfig, Store, UiSettings};
use tauri::menu::{CheckMenuItem, Menu, MenuBuilder, MenuItem, SubmenuBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, Wry};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as _};
use tauri_plugin_deep_link::DeepLinkExt as _;

mod core_manager;
mod diagnose;
mod mihomo_api;
mod scheduler;
mod sysproxy;
mod ws_bridge;

mod core_download;

use core_download::{resolve_core_binary, resolve_core_for, CoreBinaryInfo, InstallProgress};
use core_manager::{CoreManager, CoreStatus};
use sysproxy::{SysProxyManager, SysProxyStatus};
use ws_bridge::WsHub;

pub struct AppState {
    /// 轻量交接退出：RunEvent::Exit 时跳过清理（内核留给系统收养）。
    handoff: std::sync::atomic::AtomicBool,
    /// 安全模式：坏覆写导致内核启动失败后，跳过覆写链启动一次。
    safe_mode: std::sync::atomic::AtomicBool,
    pub store: Mutex<Store>,
    /// 当前内核管理器；内核安装后整体替换（core_slot）。
    core_slot: Mutex<CoreManager>,
    /// 启动链操作锁：引擎换核(ensure)+渲染+启动必须整体串行，否则两个并发
    /// start_core（连接开关 vs 引擎切换）会交错出「A 换好的核被 B 换走，
    /// A 拿 B 的管理器启动 A 的配置」——即 mihomo 进程配 sing-box JSON。
    core_op: Mutex<()>,
    sysproxy: SysProxyManager,
    /// 内核运行模式：direct / rule / global。
    mode: Mutex<String>,
    tray: Mutex<Option<tauri::tray::TrayIcon<Wry>>>,
    ws: WsHub,
    data_dir: PathBuf,
}

impl AppState {
    /// 当前内核管理器快照（clone 便宜：内部是 Arc）。
    pub fn core(&self) -> CoreManager {
        self.core_slot.lock().unwrap().clone()
    }
}

// ---------- 内核与配置的公共操作（命令与托盘共用） ----------

/// 渲染当前配置（记录覆写错误到 store，供 UI 展示）。
fn render_current(
    state: &AppState,
    use_overrides: bool,
) -> Result<crossbow_core::Rendered, String> {
    let mut store = state.store.lock().unwrap();
    match crossbow_core::render_config_with(store.data(), use_overrides) {
        Ok(r) => {
            store.data_mut().clear_override_errors();
            let _ = store.save();
            Ok(r)
        }
        Err(crossbow_core::RenderError::OverrideFailed { id, detail, .. }) => {
            store
                .data_mut()
                .set_override_error(&id, Some(detail.clone()));
            let _ = store.save();
            Err(format!("覆写执行失败：{detail}"))
        }
        Err(e) => Err(e.to_string()),
    }
}

/// 渲染当前档案并启动内核；已在运行时报错（用 `restart_core`）。
/// 安全模式（safe_mode）下跳过覆写链。
fn start_core(state: &AppState) -> Result<(), String> {
    let _op = state.core_op.lock().unwrap_or_else(|e| e.into_inner());
    start_core_locked(state)
}

/// 调用方必须已持有 core_op（start_core / restart_core 入口）。
fn start_core_locked(state: &AppState) -> Result<(), String> {
    let safe = state.safe_mode.load(std::sync::atomic::Ordering::SeqCst);
    let is_singbox = {
        let store = state.store.lock().unwrap();
        store.data().engine.engine == crossbow_core::Engine::SingBox
    };
    let need = if is_singbox { "sing-box" } else { "mihomo" };
    ensure_engine_binary(state, need)?;
    let rendered = render_current(state, !safe)?;
    let store = state.store.lock().unwrap();
    let rt = RuntimeConfig {
        engine: store.data().engine.engine,
        mixed_port: store.data().engine.mixed_port,
        allow_lan: store.data().engine.allow_lan,
        ..RuntimeConfig::default()
    };
    state.core().start(&rendered.config, &rt)
}

/// 管理器持有的二进制与目标引擎不一致时（引擎切换、冷启动回退），就地换核。
/// 返回换核后实际使用的二进制路径。
fn ensure_engine_binary(state: &AppState, need: &str) -> Result<PathBuf, String> {
    let cur = state.core().binary_path();
    state.core().log_decision(&format!(
        "ensure: need={need} cur={} cur_exists={}",
        cur.display(),
        cur.exists()
    ));
    if cur.exists() && cur.file_name().map(|f| f == need).unwrap_or(false) {
        return Ok(cur);
    }
    let Some(p) = core_download::resolve_core_for(&state.data_dir, Some(need)) else {
        return Err(format!(
            "内核「{need}」未安装 — 到设置页下载对应内核后再启动"
        ));
    };
    state.core().stop();
    *state.core_slot.lock().unwrap() =
        CoreManager::new(p.clone(), state.data_dir.join("runtime"));
    state.core().log_decision(&format!("ensure: swapped to {}", p.display()));
    Ok(p)
}

/// 热重启：切换/更新配置后让新配置生效。
fn restart_core(state: &AppState) -> Result<(), String> {
    let _op = state.core_op.lock().unwrap_or_else(|e| e.into_inner());
    state.core().stop();
    start_core_locked(state)
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
        last_update: None,
    }
}

fn gen_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

// ---------- Profile 命令 ----------

/// 导入 URL 订阅的共享实现（命令与深链接共用）。
fn import_url_blocking(state: &AppState, url: &str, name: Option<&str>) -> Result<Profile, String> {
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

/// 导入 URL 订阅：拉取 → 校验 → 落库。
#[tauri::command]
fn import_profile_url(
    app: AppHandle,
    state: State<AppState>,
    url: String,
    name: Option<String>,
) -> Result<Profile, String> {
    let profile = import_url_blocking(&state, &url, name.as_deref())?;
    refresh_tray(&app);
    Ok(profile)
}

/// 导入本地/粘贴的配置内容。
#[tauri::command]
fn import_profile_content(
    app: AppHandle,
    state: State<AppState>,
    name: String,
    content: String,
) -> Result<Profile, String> {
    let profile = import_content_blocking(&state, name, content)?;
    refresh_tray(&app);
    Ok(profile)
}

/// 从文件名推断 Profile 名称（取去扩展名的文件名）。
fn profile_name_from_path(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "本地配置".into())
}

/// 导入本地配置文件（文件选择器与拖拽导入共用）。
#[tauri::command]
fn import_profile_file(
    app: AppHandle,
    state: State<AppState>,
    path: String,
) -> Result<Profile, String> {
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("读取文件失败 {path}: {e}"))?;
    let profile = import_content_blocking(&state, profile_name_from_path(&path), content)
        .map_err(|e| format!("{path}: {e}"))?;
    refresh_tray(&app);
    Ok(profile)
}

/// 本地内容导入的共享实现。
fn import_content_blocking(
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

/// 刷新一次订阅（命令与调度器共用）：拉取 → 校验 → 落库（失败保留旧内容
/// 并记录结果）→ 运行中热重启。网络请求不持有 store 锁。
fn refresh_profile_blocking(state: &State<AppState>, id: &str) -> Result<Profile, String> {
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
                    detail: Some(e.to_string()),
                    at: now_secs(),
                },
            );
            return Err(e.to_string());
        }
    };
    if let Err(e) = validate_profile_content(&fetched.content) {
        let _ = state.store.lock().unwrap().data_mut().set_profile_outcome(
            id,
            crossbow_core::UpdateOutcome {
                ok: false,
                detail: Some(e.to_string()),
                at: now_secs(),
            },
        );
        return Err(e.to_string());
    }

    let was_active_and_running = {
        let store = state.store.lock().unwrap();
        store.data().active_profile.as_deref() == Some(id)
            && state.core().status() == CoreStatus::Running
    };
    let profile = {
        let mut store = state.store.lock().unwrap();
        store.data_mut().update_profile_content(
            id,
            fetched.content.clone(),
            now_secs(),
            fetched.traffic,
        )?;
        let _ = store.data_mut().set_profile_outcome(
            id,
            crossbow_core::UpdateOutcome {
                ok: true,
                detail: None,
                at: now_secs(),
            },
        );
        store.save().map_err(|e| e.to_string())?;
        store.data().profile(id).cloned().unwrap()
    };
    if was_active_and_running {
        restart_core(state)?;
    }
    Ok(profile)
}

/// 刷新订阅并落库；正在运行则热重启应用新配置。
#[tauri::command]
fn update_profile(app: AppHandle, state: State<AppState>, id: String) -> Result<Profile, String> {
    let profile = refresh_profile_blocking(&state, &id)?;
    refresh_tray(&app);
    Ok(profile)
}

/// 创建覆写（默认 Script 或 Merge）。
/// 列出全部覆写。
#[tauri::command]
async fn list_overrides(
    state: State<'_, AppState>,
) -> Result<Vec<crossbow_core::OverrideDef>, String> {
    Ok(state.store.lock().unwrap().data().overrides.clone())
}

#[tauri::command]
async fn create_override(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    kind: String,
) -> Result<crossbow_core::OverrideDef, String> {
    let kind = match kind.as_str() {
        "script" => crossbow_core::OverrideKind::Script,
        "merge" => crossbow_core::OverrideKind::Merge,
        _ => return Err(format!("unknown kind: {kind}")),
    };
    let def = crossbow_core::OverrideDef {
        id: gen_id(),
        name,
        kind,
        last_error: None,
        enabled: true,
        content: if kind == crossbow_core::OverrideKind::Script {
            "function main(config) {
  // 在此编辑覆写脚本
  return config;
}"
            .into()
        } else {
            "# merge 补丁示例：覆盖端口
# mixed-port: 7897
"
            .into()
        },
    };
    {
        let mut store = state.store.lock().unwrap();
        store.data_mut().overrides.push(def.clone());
        store.save().map_err(|e| e.to_string())?;
    }
    refresh_tray(&app);
    Ok(def)
}

/// 更新覆写内容/名称。
/// 校验当前覆写链（渲染一遍，错误记录到对应覆写）。
#[tauri::command]
async fn validate_overrides(state: State<'_, AppState>) -> Result<(), String> {
    let mut store = state.store.lock().unwrap();
    let failures = crossbow_core::validate_enabled_overrides(store.data());
    for (id, err) in &failures {
        store.data_mut().set_override_error(id, Some(err.clone()));
    }
    // 清除本次校验通过的启用覆写的错误；停用的保留原状
    for o in store.data_mut().overrides.iter_mut() {
        if o.enabled && !failures.iter().any(|(id, _)| id == &o.id) {
            o.last_error = None;
        }
    }
    store.save().map_err(|e| e.to_string())?;
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("{} 个覆写校验失败", failures.len()))
    }
}

/// 校验通过后应用覆写：复位安全模式，运行中则热重启。
#[tauri::command]
async fn apply_overrides(state: State<'_, AppState>) -> Result<(), String> {
    state
        .safe_mode
        .store(false, std::sync::atomic::Ordering::SeqCst);
    if state.core().status() == CoreStatus::Running {
        restart_core(&state)?;
    }
    Ok(())
}

#[tauri::command]
fn update_override(
    state: State<AppState>,
    id: String,
    content: Option<String>,
    name: Option<String>,
) -> Result<(), String> {
    let mut store = state.store.lock().unwrap();
    let mut content_changed = false;
    let o = store
        .data_mut()
        .overrides
        .iter_mut()
        .find(|o| o.id == id)
        .ok_or("override not found")?;
    if let Some(c) = content {
        o.content = c;
        content_changed = true;
    }
    if let Some(n) = name {
        o.name = n;
    }
    let oid = id.clone();
    store.save().map_err(|e| e.to_string())?;
    drop(store);
    if content_changed {
        let store = state.store.lock().unwrap();
        let binds_active = store
            .data()
            .active_profile
            .as_deref()
            .and_then(|pid| store.data().profile(pid))
            .map(|p| p.override_ids.contains(&oid))
            .unwrap_or(false);
        drop(store);
        if binds_active && state.core().status() == CoreStatus::Running {
            restart_core(&state)?;
        }
    }
    Ok(())
}

/// 删除覆写（并从所有 Profile 摘除引用）。
#[tauri::command]
async fn remove_override(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let mut store = state.store.lock().unwrap();
    store.data_mut().overrides.retain(|o| o.id != id);
    for p in store.data_mut().profiles.iter_mut() {
        p.override_ids.retain(|oid| oid != &id);
    }
    store.save().map_err(|e| e.to_string())?;
    drop(store);
    if state.core().status() == CoreStatus::Running {
        restart_core(&state)?;
    }
    refresh_tray(&app);
    Ok(())
}

/// 绑定/解绑覆写到 Profile。
#[tauri::command]
async fn toggle_override_binding(
    state: State<'_, AppState>,
    profile_id: String,
    override_id: String,
    enabled: bool,
) -> Result<(), String> {
    let mut store = state.store.lock().unwrap();
    let p = store
        .data_mut()
        .profiles
        .iter_mut()
        .find(|p| p.id == profile_id)
        .ok_or("profile not found")?;
    if enabled {
        if !p.override_ids.contains(&override_id) {
            p.override_ids.push(override_id);
        }
    } else {
        p.override_ids.retain(|oid| oid != &override_id);
    }
    store.save().map_err(|e| e.to_string())?;
    drop(store);
    if state.core().status() == CoreStatus::Running {
        restart_core(&state)?;
    }
    Ok(())
}

/// 切换覆写启用态。
#[tauri::command]
async fn set_override_enabled(
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    let mut store = state.store.lock().unwrap();
    let o = store
        .data_mut()
        .overrides
        .iter_mut()
        .find(|o| o.id == id)
        .ok_or("override not found")?;
    o.enabled = enabled;
    store.save().map_err(|e| e.to_string())?;
    drop(store);
    if state.core().status() == CoreStatus::Running {
        restart_core(&state)?;
    }
    Ok(())
}

/// 重命名 Profile。
#[tauri::command]
fn rename_profile(
    app: AppHandle,
    state: State<AppState>,
    id: String,
    name: String,
) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("名称不能为空".into());
    }
    {
        let mut store = state.store.lock().unwrap();
        let p = store
            .data_mut()
            .profiles
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or("profile not found")?;
        p.name = name.to_string();
        store.save().map_err(|e| e.to_string())?;
    }
    refresh_tray(&app);
    Ok(())
}

/// 节点旗帜补全开关。
/// 切换内核引擎（mihomo / singbox）。运行中则热重启；切换目标未安装则报错。
#[tauri::command]
async fn set_engine(
    app: AppHandle,
    state: State<'_, AppState>,
    engine: String,
) -> Result<(), String> {
    let target = match engine.as_str() {
        "mihomo" => crossbow_core::Engine::Mihomo,
        "singbox" => crossbow_core::Engine::SingBox,
        _ => return Err(format!("unknown engine: {engine}")),
    };
    let need_bin = match target {
        crossbow_core::Engine::SingBox => "sing-box",
        crossbow_core::Engine::Mihomo => "mihomo",
    };
    if !core_download::resolve_core_for(&state.data_dir, Some(&engine)).is_some() {
        return Err(format!("内核 {need_bin} 未安装，请先在设置页下载"));
    }
    {
        let mut store = state.store.lock().unwrap();
        store.data_mut().engine.engine = target;
        store.save().map_err(|e| e.to_string())?;
    }
    state.core().log_decision(&format!(
        "set_engine: -> {engine} status={:?}",
        state.core().status()
    ));
    // Running 热重启；Starting（上次尝试还在探测）也要收掉后按新引擎拉起
    if matches!(
        state.core().status(),
        CoreStatus::Running | CoreStatus::Starting
    ) {
        restart_core(&state)?;
    }
    refresh_tray(&app);
    // 通知侧栏 footer 等跟随引擎的 UI 重新取标识
    let _ = app.emit("engine://changed", engine.clone());
    Ok(())
}

#[tauri::command]
fn set_flag_emoji(state: State<AppState>, enabled: bool) -> Result<(), String> {
    let mut store = state.store.lock().unwrap();
    store.data_mut().engine.flag_emoji = enabled;
    store.save().map_err(|e| e.to_string())?;
    Ok(())
}

/// 设置订阅自动更新间隔（分钟，0 = 关闭）。
#[tauri::command]
fn set_profile_interval(
    state: State<AppState>,
    id: String,
    interval_min: u32,
) -> Result<(), String> {
    let mut store = state.store.lock().unwrap();
    let p = store
        .data_mut()
        .profiles
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or("profile not found")?;
    p.update_interval_min = interval_min;
    store.save().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn set_active_profile(app: AppHandle, state: State<AppState>, id: String) -> Result<(), String> {
    let running = state.core().status() == CoreStatus::Running;
    {
        let mut store = state.store.lock().unwrap();
        store.data_mut().set_active_profile(&id)?;
        store.save().map_err(|e| e.to_string())?;
    }
    if running {
        restart_core(&state)?;
    }
    refresh_tray(&app);
    Ok(())
}

#[tauri::command]
fn remove_profile(app: AppHandle, state: State<AppState>, id: String) -> Result<(), String> {
    {
        let mut store = state.store.lock().unwrap();
        store.data_mut().remove_profile(&id)?;
        store.save().map_err(|e| e.to_string())?;
    }
    refresh_tray(&app);
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

// ---------- 内核与系统代理命令 ----------

#[tauri::command]
fn core_start(state: State<AppState>) -> Result<(), String> {
    start_core(&state)
}

#[tauri::command]
fn core_stop(state: State<AppState>) -> Result<(), String> {
    state.core().stop();
    Ok(())
}

#[tauri::command]
fn core_status(state: State<AppState>) -> CoreStatus {
    state.core().status()
}

// ---------- WS 数据桥命令（页面挂载订阅、卸载退订） ----------

fn require_controller(state: &AppState) -> Result<(u16, String), String> {
    state
        .core()
        .controller()
        .ok_or_else(|| "内核未运行".to_string())
}

#[tauri::command]
fn subscribe_traffic(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    let (port, secret) = require_controller(&state)?;
    let h = app.clone();
    state
        .ws
        .subscribe_traffic(port, &secret, move |ev, payload| {
            let _ = h.emit(ev, payload);
        })
}

#[tauri::command]
fn unsubscribe_traffic(state: State<AppState>) -> Result<(), String> {
    state.ws.unsubscribe("traffic");
    Ok(())
}

#[tauri::command]
fn subscribe_connections(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    let (port, secret) = require_controller(&state)?;
    let h = app.clone();
    state
        .ws
        .subscribe_connections(port, &secret, move |ev, payload: serde_json::Value| {
            let _ = h.emit(ev, payload);
        })
}

#[tauri::command]
fn unsubscribe_connections(state: State<AppState>) -> Result<(), String> {
    state.ws.unsubscribe("connections");
    Ok(())
}

#[tauri::command]
fn subscribe_logs(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    let (port, secret) = require_controller(&state)?;
    let h = app.clone();
    state
        .ws
        .subscribe_logs(port, &secret, move |ev, payload: serde_json::Value| {
            let _ = h.emit(ev, payload);
        })
}

#[tauri::command]
fn unsubscribe_logs(state: State<AppState>) -> Result<(), String> {
    state.ws.unsubscribe("logs");
    Ok(())
}

// ---------- 诊断 ----------

#[tauri::command]
fn run_diagnosis(state: State<AppState>) -> Vec<diagnose::CheckResult> {
    diagnose::run(&state)
}

#[tauri::command]
fn current_outbound(state: State<AppState>) -> Result<mihomo_api::OutboundInfo, String> {
    let fallback = state.core().current_config().and_then(|c| extract_final_outbound(&c));
    let c = controller_client(&state)?;
    c.current_outbound(fallback.as_deref())
}

/// 从最终配置提取兜底出口目标：mihomo YAML 取 MATCH 行的 target；
/// sing-box JSON 取 route.final。/rules 无 MATCH 规则时（sing-box）用它。
fn extract_final_outbound(cfg: &str) -> Option<String> {
    if cfg.trim_start().starts_with('{') {
        let v: serde_json::Value = serde_json::from_str(cfg).ok()?;
        v.get("route")?.get("final")?.as_str().map(String::from)
    } else {
        cfg.lines()
            .rev()
            .find(|l| l.trim_start().to_lowercase().starts_with("match,"))
            .and_then(|l| l.split(',').nth(1))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }
}

#[tauri::command]
fn close_connection(state: State<AppState>, id: String) -> Result<(), String> {
    controller_client(&state)?.close_connection(&id)
}

/// 前端黑匣子：全局错误/心跳落盘，用于定位 webview 白屏类问题。
#[tauri::command]
fn diag_log(state: State<AppState>, line: String) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state.data_dir.join("webview.log"))
    {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "{ts} {line}");
    }
}

// ---------- 内核安装命令 ----------

/// 按引擎查询内核安装状态。
#[tauri::command]
async fn core_binary_info_for(
    state: State<'_, AppState>,
    engine: String,
) -> Result<CoreBinaryInfo, String> {
    let resolved = core_download::resolve_core_for(&state.data_dir, Some(&engine));
    let path = resolved
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let source = match &resolved {
        None => "missing",
        Some(p) if p.starts_with(&state.data_dir) => "data",
        Some(_) => "builtin",
    };
    let version = resolved
        .as_ref()
        .and_then(core_version_of)
        .unwrap_or_default();
    Ok(CoreBinaryInfo {
        path,
        source: source.into(),
        version,
    })
}

#[tauri::command]
fn core_binary_info(state: State<AppState>) -> CoreBinaryInfo {
    let resolved = resolve_core_binary(&state.data_dir);
    let path = resolved
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let source = match &resolved {
        None => "missing",
        Some(p) if p.starts_with(&state.data_dir) => "data",
        Some(_) => "builtin",
    };
    let version = resolved
        .as_ref()
        .and_then(core_version_of)
        .unwrap_or_default();
    CoreBinaryInfo {
        path,
        source: source.into(),
        version,
    }
}

/// 下载安装内核（前端用 invoke 阻塞调用，期间显示进度提示）。
#[tauri::command]
async fn core_install(app: AppHandle, engine: Option<String>) -> Result<CoreBinaryInfo, String> {
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let kind = match engine.as_deref() {
        Some("singbox") => core_download::CoreKind::SingBox,
        _ => core_download::CoreKind::Mihomo,
    };
    let h = app.clone();
    let info = core_download::install_core(&data_dir, kind, &move |p: InstallProgress| {
        let _ = h.emit("core://install-progress", p);
    })?;
    // 安装后重新装载 CoreManager 的二进制路径：直接替换状态里的管理器。
    let state = app.state::<AppState>();
    // 与启动链互斥：换核期间不允许并发的 start_core 拿到半新半旧的槽位
    let _op = state.core_op.lock().unwrap_or_else(|e| e.into_inner());
    let new_core = CoreManager::new(info.clone(), data_dir.join("runtime"));
    state.core().stop();
    *state.core_slot.lock().unwrap() = new_core;
    Ok(CoreBinaryInfo {
        path: info.display().to_string(),
        source: "data".into(),
        version: core_version_of(&info).unwrap_or_default(),
    })
}

/// 内核版本（执行 `<bin> -v` 首行）；sing-box 只认 `version` 子命令，依次尝试。
fn core_version_of(bin: &PathBuf) -> Option<String> {
    for flag in ["-v", "version"] {
        let Ok(out) = std::process::Command::new(bin).arg(flag).output() else {
            continue;
        };
        if !out.status.success() {
            continue;
        }
        let first = String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        if !first.is_empty() {
            return Some(first);
        }
    }
    None
}

// ---------- 代理组命令（代理页） ----------

fn controller_client(state: &AppState) -> Result<mihomo_api::Controller, String> {
    let (port, secret) = state
        .core()
        .controller()
        .ok_or_else(|| "内核未运行".to_string())?;
    Ok(mihomo_api::Controller { port, secret })
}

#[tauri::command]
fn proxies_snapshot(state: State<AppState>) -> Result<Vec<mihomo_api::GroupView>, String> {
    controller_client(&state)?.groups()
}

#[tauri::command]
fn select_proxy(state: State<AppState>, group: String, name: String) -> Result<(), String> {
    controller_client(&state)?.select_proxy(&group, &name)
}

#[tauri::command]
fn test_group_delay(
    state: State<AppState>,
    group: String,
) -> Result<BTreeMap<String, u64>, String> {
    controller_client(&state)?.test_group_delay(&group)
}

#[tauri::command]
fn test_node_delay(state: State<AppState>, node: String) -> Result<u64, String> {
    controller_client(&state)?.test_node_delay(&node)
}

// ---------- 设置命令 ----------

#[tauri::command]
fn get_ui_settings(state: State<AppState>) -> UiSettings {
    state.store.lock().unwrap().data().ui.clone()
}

#[derive(serde::Serialize)]
struct EngineView {
    engine: String,
    mixed_port: u16,
    allow_lan: bool,
    flag_emoji: bool,
    /// 当前引擎对应内核是否已安装。
    core_installed: bool,
}

#[tauri::command]
fn get_engine_config(state: State<AppState>) -> EngineView {
    let store = state.store.lock().unwrap();
    let need = match store.data().engine.engine {
        crossbow_core::Engine::SingBox => "sing-box",
        crossbow_core::Engine::Mihomo => "mihomo",
    };
    let core_installed = core_download::resolve_core_for(&state.data_dir, Some(need))
        .map(|p| p.file_name().map(|f| f == need).unwrap_or(false))
        .unwrap_or(false);
    EngineView {
        engine: format!("{:?}", store.data().engine.engine).to_lowercase(),
        mixed_port: store.data().engine.mixed_port,
        allow_lan: store.data().engine.allow_lan,
        flag_emoji: store.data().engine.flag_emoji,
        core_installed,
    }
}

#[tauri::command]
fn set_theme(app: AppHandle, state: State<AppState>, theme: String) -> Result<(), String> {
    if !["system", "light", "dark"].contains(&theme.as_str()) {
        return Err(format!("invalid theme: {theme}"));
    }
    {
        let mut store = state.store.lock().unwrap();
        store.data_mut().ui.theme = theme.clone();
        store.save().map_err(|e| e.to_string())?;
    }
    let _ = app.emit("ui://theme", theme);
    Ok(())
}

#[tauri::command]
fn set_lightweight_close(state: State<AppState>, enabled: bool) -> Result<(), String> {
    let mut store = state.store.lock().unwrap();
    store.data_mut().ui.lightweight_close = enabled;
    store.save().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn set_lang(app: AppHandle, state: State<AppState>, lang: String) -> Result<(), String> {
    if !["zh", "en"].contains(&lang.as_str()) {
        return Err(format!("invalid lang: {lang}"));
    }
    {
        let mut store = state.store.lock().unwrap();
        store.data_mut().ui.lang = lang.clone();
        store.save().map_err(|e| e.to_string())?;
    }
    let _ = app.emit("ui://lang", lang);
    refresh_tray(&app);
    Ok(())
}

/// 修改混合端口：持久化 → 运行中则热重启 → 系统代理开启则跟随换端口。
#[tauri::command]
fn set_mixed_port(app: AppHandle, state: State<AppState>, port: u16) -> Result<(), String> {
    if port < 1024 {
        return Err("端口不能小于 1024".into());
    }
    let proxy_was_on = state.sysproxy.status().enabled;
    if proxy_was_on {
        state.sysproxy.disable();
    }
    {
        let mut store = state.store.lock().unwrap();
        store.data_mut().engine.mixed_port = port;
        store.save().map_err(|e| e.to_string())?;
    }
    if state.core().status() == CoreStatus::Running {
        restart_core(&state)?;
    }
    if proxy_was_on {
        state.sysproxy.enable(port)?;
        let _ = app.emit("sysproxy://status", state.sysproxy.status());
    }
    Ok(())
}

#[tauri::command]
fn set_allow_lan(state: State<AppState>, enabled: bool) -> Result<(), String> {
    {
        let mut store = state.store.lock().unwrap();
        store.data_mut().engine.allow_lan = enabled;
        store.save().map_err(|e| e.to_string())?;
    }
    if state.core().status() == CoreStatus::Running {
        restart_core(&state)?;
    }
    Ok(())
}

#[tauri::command]
fn autostart_status(app: AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
fn autostart_set(app: AppHandle, enable: bool) -> Result<(), String> {
    let mgr = app.autolaunch();
    if enable {
        mgr.enable().map_err(|e| e.to_string())
    } else {
        mgr.disable().map_err(|e| e.to_string())
    }
}

/// 内核版本（执行 `<bin> -v` 首行）。
#[tauri::command]
fn core_version(state: State<AppState>) -> Result<String, String> {
    let bin = state.core().binary_path();
    if !bin.exists() {
        return Err("内核未安装".into());
    }
    let out = std::process::Command::new(&bin)
        .arg("-v")
        .output()
        .map_err(|e| e.to_string())?;
    let first = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    if first.is_empty() {
        Err("无法读取内核版本".into())
    } else {
        Ok(first)
    }
}

// ---------- 深链接 ----------

/// `crossbow://import?url=<订阅地址>`：后台导入并通知前端。
fn handle_crossbow_url(app: &AppHandle, raw: &str) {
    let Ok(u) = url::Url::parse(raw) else { return };
    if u.host_str() != Some("import") {
        return;
    }
    let Some(target) = u
        .query_pairs()
        .find(|(k, _)| k == "url")
        .map(|(_, v)| v.to_string())
    else {
        return;
    };
    let app = app.clone();
    std::thread::spawn(move || {
        show_main_window(&app);
        let state = app.state::<AppState>();
        match import_url_blocking(&state, &target, None) {
            Ok(_) => {
                let _ = app.emit("profile://imported", target);
                refresh_tray(&app);
            }
            Err(e) => {
                let _ = app.emit("profile://import-failed", format!("{target}: {e}"));
            }
        }
    });
}

#[tauri::command]
fn sysproxy_status(state: State<AppState>) -> SysProxyStatus {
    state.sysproxy.status()
}

/// 总开关：开启 = 确保内核运行 + 设置系统代理；关闭 = 还原系统代理（内核保持）。
#[tauri::command]
fn sysproxy_toggle(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    if state.sysproxy.status().enabled {
        state.sysproxy.disable();
    } else {
        let port = {
            let store = state.store.lock().unwrap();
            store.data().engine.mixed_port
        };
        if !matches!(state.core().status(), CoreStatus::Running) {
            start_core(&state)?;
        }
        state.sysproxy.enable(port)?;
    }
    let _ = app.emit("sysproxy://status", state.sysproxy.status());
    refresh_tray(&app);
    Ok(())
}

#[tauri::command]
fn core_mode(state: State<AppState>) -> String {
    state.mode.lock().unwrap().clone()
}

/// 切换运行模式（经控制器 PATCH，运行时生效）。
#[tauri::command]
fn set_core_mode(app: AppHandle, state: State<AppState>, mode: String) -> Result<(), String> {
    if let Some((port, secret)) = state.core().controller() {
        mihomo_api::Controller { port, secret }.patch_mode(&mode)?;
    }
    *state.mode.lock().unwrap() = mode.clone();
    let _ = app.emit("core://mode", mode);
    refresh_tray(&app);
    Ok(())
}

// ---------- 托盘 ----------

/// 托盘文案的双语标签（语言跟随设置页）。
fn tr(lang: &str, zh: &str, en: &str) -> String {
    if lang == "en" {
        en.to_string()
    } else {
        zh.to_string()
    }
}

fn build_tray_menu(app: &AppHandle) -> Result<Menu<Wry>, tauri::Error> {
    let state = app.state::<AppState>();
    let lang = state.store.lock().unwrap().data().ui.lang.clone();

    let sp = state.sysproxy.status();
    let proxy_label = if sp.enabled {
        format!(
            "{} :{}",
            tr(&lang, "系统代理：已开启", "System Proxy: On"),
            sp.port
        )
    } else {
        tr(&lang, "系统代理：已关闭", "System Proxy: Off")
    };
    let proxy = MenuItem::with_id(app, "toggle-proxy", &proxy_label, true, None::<&str>)?;

    let mode = state.mode.lock().unwrap().clone();
    let mk_check = |id: &str, label: &str, checked: bool| {
        CheckMenuItem::with_id(app, id, label, true, checked, None::<&str>)
    };
    let mode_menu = SubmenuBuilder::new(app, tr(&lang, "模式", "Mode"))
        .item(&mk_check(
            "mode-direct",
            &tr(&lang, "直连", "Direct"),
            mode == "direct",
        )?)
        .item(&mk_check(
            "mode-rule",
            &tr(&lang, "规则", "Rule"),
            mode == "rule",
        )?)
        .item(&mk_check(
            "mode-global",
            &tr(&lang, "全局", "Global"),
            mode == "global",
        )?)
        .build()?;

    let (profiles, active) = {
        let store = state.store.lock().unwrap();
        (
            store.data().profiles.clone(),
            store.data().active_profile.clone(),
        )
    };
    let mut profile_builder = SubmenuBuilder::new(app, tr(&lang, "配置", "Profiles"));
    for p in &profiles {
        profile_builder = profile_builder.item(&mk_check(
            &format!("profile-{}", p.id),
            &p.name,
            active.as_deref() == Some(p.id.as_str()),
        )?);
    }
    let profile_menu = profile_builder.build()?;

    let open = MenuItem::with_id(
        app,
        "open",
        tr(&lang, "打开主窗口", "Open Window"),
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(
        app,
        "quit",
        tr(&lang, "退出（还原系统代理）", "Quit (restore proxy)"),
        true,
        None::<&str>,
    )?;

    MenuBuilder::new(app)
        .item(&proxy)
        .separator()
        .item(&mode_menu)
        .item(&profile_menu)
        .separator()
        .item(&open)
        .separator()
        .item(&quit)
        .build()
}

fn refresh_tray(app: &AppHandle) {
    if let Some(tray) = app.state::<AppState>().tray.lock().unwrap().as_ref() {
        match build_tray_menu(app) {
            Ok(menu) => {
                let _ = tray.set_menu(Some(menu));
            }
            Err(e) => eprintln!("rebuild tray menu: {e}"),
        }
    }
}

fn show_main_window(app: &AppHandle) {
    match app.get_webview_window("main") {
        Some(win) => {
            let _ = win.show();
            let _ = win.set_focus();
        }
        None => {
            // 轻量待机中窗口已销毁：重建
            let _ = tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title("Crossbow")
            .inner_size(1120.0, 720.0)
            .min_inner_size(880.0, 560.0)
            .center()
            // 与 tauri.conf.json 一致：Overlay 去掉原生标题栏，内容延伸到窗口顶，
            // 避免标题栏材质在深色 UI 下出现灰白边/白条（NSWindow.setBackgroundColor 会毁标题栏外观，勿用）
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .build();
            // 内核仍是本进程的子进程（adopted/own），继续归我们管
            if let Some(state) = app.try_state::<AppState>() {
                state
                    .handoff
                    .store(false, std::sync::atomic::Ordering::SeqCst);
            }
            let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
        }
    }
}

/// 系统代理总开关（托盘与首页共用）。
fn toggle_sysproxy(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    state.core().log_decision(&format!(
        "toggle_sysproxy: enabled={} status={:?}",
        state.sysproxy.status().enabled,
        state.core().status()
    ));
    if state.sysproxy.status().enabled {
        state.sysproxy.disable();
    } else {
        let port = {
            let store = state.store.lock().unwrap();
            store.data().engine.mixed_port
        };
        if !matches!(state.core().status(), CoreStatus::Running) {
            start_core(&state)?;
        }
        state.sysproxy.enable(port)?;
    }
    let _ = app.emit("sysproxy://status", state.sysproxy.status());
    refresh_tray(app);
    Ok(())
}

/// 轻量模式关窗：内核交接留核（GUI 进程保留，托盘常驻可唤醒）。
/// 主窗口销毁以释放 webview 内存；ws 订阅随窗口销毁一并退订。
fn lightweight_quit(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Some(_pid) = state.core().detach() else {
        // 内核未运行：没有可交接的东西，等价于普通关窗（隐藏到托盘）
        show_no_more(app);
        return;
    };
    state
        .handoff
        .store(true, std::sync::atomic::Ordering::SeqCst);
    state.ws.unsubscribe_all();
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.destroy(); // 直接销毁，绕过 CloseRequested 防递归
    }
    // Dock 图标隐藏（仅剩顶部状态栏托盘）；cmd+tab 切换能力保留
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
}

fn show_no_more(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.hide();
    }
}

fn handle_menu(app: &AppHandle, event: tauri::menu::MenuEvent) {
    let id = event.id().as_ref();
    let result = match id {
        "toggle-proxy" => toggle_sysproxy(app),
        "mode-direct" | "mode-rule" | "mode-global" => {
            let mode = id.strip_prefix("mode-").unwrap().to_string();
            set_core_mode(app.clone(), app.state::<AppState>(), mode)
        }
        "open" => {
            show_main_window(app);
            Ok(())
        }
        "quit" => {
            if let Some(state) = app.try_state::<AppState>() {
                state
                    .handoff
                    .store(false, std::sync::atomic::Ordering::SeqCst);
            }
            app.exit(0);
            Ok(())
        }
        "lightweight-quit" => {
            lightweight_quit(app);
            Ok(())
        }
        other => match other.strip_prefix("profile-") {
            Some(pid) => set_active_profile(app.clone(), app.state::<AppState>(), pid.to_string()),
            None => Ok(()),
        },
    };
    if let Err(e) = result {
        eprintln!("tray action {id}: {e}");
    }
}

// ---------- 应用入口 ----------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // 单实例必须最先注册：二次启动的 argv 里可能带 crossbow:// 深链接。
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            show_main_window(app);
            if let Some(url) = argv.iter().find(|a| a.starts_with("crossbow://")) {
                handle_crossbow_url(app, url);
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;

            // 双实例保险：single-instance 插件依赖对端实例的事件循环还活着
            // （主线程卡死/退出被误拦的旧实例接不住通知）。pidfile 不依赖对方，
            // 只要 pid 存活就拒绝第二个实例，避免两实例互杀内核。
            let lock_path = data_dir.join("app.pid");
            if let Ok(old) = std::fs::read_to_string(&lock_path) {
                if let Ok(pid) = old.trim().parse::<i32>() {
                    if unsafe { libc::kill(pid, 0) } == 0 {
                        return Err(format!(
                            "另一个 Crossbow 实例正在运行 (pid {pid})，退出它后再启动"
                        )
                        .into());
                    }
                }
            }
            std::fs::write(&lock_path, std::process::id().to_string())?;

            let store = Store::open(&data_dir)
                .map_err(|e| std::io::Error::other(format!("open store: {e}")))?;

            // 内核二进制定位：优先按持久化引擎精确解析；引擎对应内核缺失时
            // 回退任意可用链（env → sidecar → 数据目录），启动时仍会校验匹配。
            let engine_key = match store.data().engine.engine {
                crossbow_core::Engine::SingBox => "singbox",
                crossbow_core::Engine::Mihomo => "mihomo",
            };
            let bin = resolve_core_for(&data_dir, Some(engine_key))
                .or_else(|| resolve_core_binary(&data_dir));
            let core = CoreManager::new(bin.unwrap_or_default(), data_dir.join("runtime"));

            let handle = app.handle().clone();
            core.set_callback(std::sync::Arc::new(move |status| {
                let _ = handle.emit("core://status", status.clone());
                // 状态一致性守护：内核崩溃时若系统代理还开着，立即还原，
                // 避免系统流量指向已死的代理端口导致用户断网。
                if let CoreStatus::Crashed(msg) = status {
                    let Some(state) = handle.try_state::<AppState>() else { return };
                    // 状态一致性守护：内核崩溃时还原系统代理，避免断网。
                    if state.sysproxy.status().enabled {
                        state.sysproxy.disable();
                        let _ = handle.emit("sysproxy://status", state.sysproxy.status());
                        refresh_tray(&handle);
                    }
                    // 安全模式：覆写引发的配置解析错误 → 禁用全部覆写并重启一次。
                    let config_err = msg.contains("Parse config error");
                    let already = state
                        .safe_mode
                        .load(std::sync::atomic::Ordering::SeqCst);
                    if config_err && !already {
                        state
                            .safe_mode
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                        {
                            let mut store = state.store.lock().unwrap();
                            for o in store.data_mut().overrides.iter_mut() {
                                o.enabled = false;
                            }
                            let _ = store.save();
                        }
                        let port = state.store.lock().unwrap().data().engine.mixed_port;
                        let _ = start_core(&state);
                        // 等内核起来后恢复系统代理（最多 20s）
                        for _ in 0..100 {
                            if state.core().status() == CoreStatus::Running {
                                let _ = state.sysproxy.enable(port);
                                let _ = handle.emit(
                                    "sysproxy://status",
                                    state.sysproxy.status(),
                                );
                                break;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(200));
                        }
                        let _ = handle.emit(
                            "core://safe-mode",
                            "覆写导致内核启动失败，已临时禁用全部覆写（安全模式）。修复覆写后保存即恢复正常。",
                        );
                        refresh_tray(&handle);
                    }
                }
            }));

            // 状态一致性守护：
            // 1) 尝试收养上一实例轻量交接的内核（描述符 + 控制器探测）
            // 2) 收养成功 → 系统代理状态从 journal 恢复（不还原设置）
            // 3) 收养失败 → 按异常退出处理：全量还原系统代理
            #[cfg(target_os = "macos")]
            let backend = sysproxy::NetworkSetup;
            #[cfg(not(target_os = "macos"))]
            let backend = sysproxy::UnsupportedBackend;
            let sysproxy = SysProxyManager::new(backend, data_dir.join("sysproxy-journal.json"));
            let adopted = core.adopt_external();
            if adopted {
                eprintln!("adopted detached core from previous lightweight session");
                if let Some(port) = sysproxy.adopt_from_journal() {
                    eprintln!("system proxy state restored :{port}");
                }
            } else if sysproxy.recover_stale() {
                eprintln!("recovered stale system proxy from previous session");
            }

            app.manage(AppState {
                handoff: std::sync::atomic::AtomicBool::new(false),
                safe_mode: std::sync::atomic::AtomicBool::new(false),
                store: Mutex::new(store),
                core_slot: Mutex::new(core),
                core_op: Mutex::new(()),
                sysproxy,
                mode: Mutex::new("rule".to_string()),
                tray: Mutex::new(None),
                ws: WsHub::default(),
                data_dir: data_dir.clone(),
            });

            // 订阅自动更新调度器
            scheduler::spawn(app.handle().clone());

            // 深链接：crossbow://import?url=...
            {
                let handle = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        handle_crossbow_url(&handle, url.as_str());
                    }
                });
            }

            // 托盘
            let menu = build_tray_menu(app.handle())?;
            let tray = TrayIconBuilder::new()
                .icon(
                    app.default_window_icon()
                        .expect("missing bundle icon")
                        .clone(),
                )
                .menu(&menu)
                .show_menu_on_left_click(true)
                .tooltip("Crossbow")
                .on_menu_event(handle_menu)
                .build(app)?;
            *app.state::<AppState>().tray.lock().unwrap() = Some(tray);

            Ok(())
        })
        .on_window_event(|window, event| {
            // 关窗：默认驻留托盘；开启「轻量模式」时交接退出（留核）。
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                let lightweight = app
                    .try_state::<AppState>()
                    .map(|s| s.store.lock().unwrap().data().ui.lightweight_close)
                    .unwrap_or(false);
                if lightweight {
                    lightweight_quit(app);
                } else {
                    let _ = window.hide();
                }
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_profiles,
            active_profile_id,
            import_profile_url,
            import_profile_content,
            import_profile_file,
            update_profile,
            set_profile_interval,
            rename_profile,
            set_flag_emoji,
            create_override,
            update_override,
            remove_override,
            toggle_override_binding,
            validate_overrides,
            apply_overrides,
            set_override_enabled,
            list_overrides,
            set_active_profile,
            remove_profile,
            core_start,
            core_stop,
            core_status,
            sysproxy_status,
            sysproxy_toggle,
            core_mode,
            set_core_mode,
            subscribe_traffic,
            unsubscribe_traffic,
            subscribe_connections,
            unsubscribe_connections,
            subscribe_logs,
            unsubscribe_logs,
            get_ui_settings,
            get_engine_config,
            set_theme,
            set_lang,
            set_lightweight_close,
            set_mixed_port,
            set_allow_lan,
            autostart_status,
            autostart_set,
            core_version,
            core_binary_info,
            core_binary_info_for,
            core_install,
            set_engine,
            diag_log,
            run_diagnosis,
            current_outbound,
            close_connection,
            proxies_snapshot,
            select_proxy,
            test_group_delay,
            test_node_delay,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            match event {
                // 轻量待机：最后一个窗口销毁会触发 ExitRequested，
                // handoff 时阻止退出（进程保留、托盘常驻、内核照跑）。
                // swap 一次性消费：只拦截「轻量关窗」这一次请求，用完即复位，
                // 否则粘性标志会连后续 cmd-Q / quit 事件一起拦死，进程永远杀不掉。
                tauri::RunEvent::ExitRequested {
                    code: None, api, ..
                } => {
                    if let Some(state) = app.try_state::<AppState>() {
                        if state
                            .handoff
                            .swap(false, std::sync::atomic::Ordering::SeqCst)
                        {
                            api.prevent_exit();
                        }
                    }
                }
                // 真正退出：还原系统代理、收内核（handoff 之外的所有路径）。
                tauri::RunEvent::Exit => {
                    if let Some(state) = app.try_state::<AppState>() {
                        state.ws.unsubscribe_all();
                        let _ = std::fs::remove_file(state.data_dir.join("app.pid"));
                        let handoff = state.handoff.load(std::sync::atomic::Ordering::SeqCst);
                        if !handoff {
                            state.sysproxy.disable();
                            state.core().stop();
                        }
                    }
                }
                _ => {}
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_name_from_path_strips_extension() {
        assert_eq!(profile_name_from_path("/a/b/my-config.yaml"), "my-config");
        assert_eq!(profile_name_from_path("配置.yml"), "配置");
        assert_eq!(profile_name_from_path("noext"), "noext");
        assert_eq!(profile_name_from_path("/a/b/.hidden"), ".hidden");
    }
}
