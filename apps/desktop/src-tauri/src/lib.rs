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
mod mihomo_api;
mod sysproxy;
mod ws_bridge;

mod core_download;

use core_download::{resolve_core_binary, CoreBinaryInfo, InstallProgress};
use core_manager::{CoreManager, CoreStatus};
use sysproxy::{SysProxyManager, SysProxyStatus};
use ws_bridge::WsHub;

struct AppState {
    store: Mutex<Store>,
    /// 当前内核管理器；内核安装后整体替换（core_slot）。
    core_slot: Mutex<CoreManager>,
    sysproxy: SysProxyManager,
    /// 内核运行模式：direct / rule / global。
    mode: Mutex<String>,
    tray: Mutex<Option<tauri::tray::TrayIcon<Wry>>>,
    ws: WsHub,
    app: AppHandle,
    data_dir: PathBuf,
}

impl AppState {
    /// 当前内核管理器快照（clone 便宜：内部是 Arc）。
    fn core(&self) -> CoreManager {
        self.core_slot.lock().unwrap().clone()
    }
}

// ---------- 内核与配置的公共操作（命令与托盘共用） ----------

/// 渲染当前档案并启动内核；已在运行时报错（用 `restart_core`）。
fn start_core(state: &AppState) -> Result<(), String> {
    let store = state.store.lock().unwrap();
    let rendered = crossbow_core::render_config(store.data()).map_err(|e| e.to_string())?;
    let rt = RuntimeConfig {
        mixed_port: store.data().engine.mixed_port,
        allow_lan: store.data().engine.allow_lan,
        ..RuntimeConfig::default()
    };
    state.core().start(&rendered.config, &rt)
}

/// 热重启：切换/更新配置后让新配置生效。
fn restart_core(state: &AppState) -> Result<(), String> {
    state.core().stop();
    start_core(state)
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

// ---------- Profile 命令 ----------

/// 导入 URL 订阅的共享实现（命令与深链接共用）。
fn import_url_blocking(state: &AppState, url: &str, name: Option<&str>) -> Result<Profile, String> {
    let fetched = fetch_subscription(url, 30).map_err(|e| e.to_string())?;
    validate_profile_content(&fetched.content).map_err(|e| e.to_string())?;
    let fallback = url.split('/').next_back().unwrap_or("订阅").to_string();
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

/// 刷新订阅并落库；正在运行则热重启应用新配置。
#[tauri::command]
fn update_profile(app: AppHandle, state: State<AppState>, id: String) -> Result<Profile, String> {
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

    let was_active_and_running = {
        let store = state.store.lock().unwrap();
        store.data().active_profile.as_deref() == Some(id.as_str())
            && state.core().status() == CoreStatus::Running
    };
    let profile = {
        let mut store = state.store.lock().unwrap();
        store
            .data_mut()
            .update_profile_content(id.as_str(), fetched.content, now_secs(), fetched.traffic)
            .map_err(|e| e.to_string())?;
        store.save().map_err(|e| e.to_string())?;
        store.data().profile(&id).cloned().unwrap()
    };
    if was_active_and_running {
        restart_core(&state)?;
    }
    refresh_tray(&app);
    Ok(profile)
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
        .subscribe_connections(port, &secret, move |ev, payload| {
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
    state.ws.subscribe_logs(port, &secret, move |ev, payload| {
        let _ = h.emit(ev, payload);
    })
}

#[tauri::command]
fn unsubscribe_logs(state: State<AppState>) -> Result<(), String> {
    state.ws.unsubscribe("logs");
    Ok(())
}

// ---------- 内核安装命令 ----------

#[tauri::command]
fn core_binary_info(state: State<AppState>) -> CoreBinaryInfo {
    let resolved = resolve_core_binary(&state.app, &state.data_dir);
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
fn core_install(app: AppHandle) -> Result<CoreBinaryInfo, String> {
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let h = app.clone();
    let info = core_download::install_core(&data_dir, &move |p: InstallProgress| {
        let _ = h.emit("core://install-progress", p);
    })?;
    // 安装后重新装载 CoreManager 的二进制路径：直接替换状态里的管理器。
    let state = app.state::<AppState>();
    let new_core = CoreManager::new(info.clone(), data_dir.join("runtime"));
    state.core().stop();
    *state.core_slot.lock().unwrap() = new_core;
    let resolved = resolve_core_binary(&app, &data_dir);
    let source = match &resolved {
        None => "missing",
        Some(p) if p.starts_with(&data_dir) => "data",
        Some(_) => "builtin",
    };
    Ok(CoreBinaryInfo {
        path: info.display().to_string(),
        source: source.into(),
        version: resolved
            .as_ref()
            .and_then(core_version_of)
            .unwrap_or_default(),
    })
}

fn core_version_of(bin: &PathBuf) -> Option<String> {
    let out = std::process::Command::new(bin).arg("-v").output().ok()?;
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()?
            .trim()
            .to_string(),
    )
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
}

#[tauri::command]
fn get_engine_config(state: State<AppState>) -> EngineView {
    let store = state.store.lock().unwrap();
    EngineView {
        engine: format!("{:?}", store.data().engine.engine).to_lowercase(),
        mixed_port: store.data().engine.mixed_port,
        allow_lan: store.data().engine.allow_lan,
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
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// 系统代理总开关（托盘与首页共用）。
fn toggle_sysproxy(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
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
            app.exit(0);
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
            let store = Store::open(&data_dir)
                .map_err(|e| std::io::Error::other(format!("open store: {e}")))?;

            // 内核二进制定位（按序）：
            // 1. CROSSBOW_MIHOMO_BIN 环境变量（开发/调试）
            // 2. 打包 .app 内置 sidecar（与可执行文件同目录）
            // 3. 数据目录缓存 binaries/mihomo（此前手动安装/下载的）
            // 都没有时为空——由「下载内核」命令补齐后再启动。
            let bin = resolve_core_binary(app.handle(), &data_dir);
            let core = CoreManager::new(bin.unwrap_or_default(), data_dir.join("runtime"));

            let handle = app.handle().clone();
            core.set_callback(std::sync::Arc::new(move |status| {
                let _ = handle.emit("core://status", status.clone());
                // 状态一致性守护：内核崩溃时若系统代理还开着，立即还原，
                // 避免系统流量指向已死的代理端口导致用户断网。
                if matches!(status, CoreStatus::Crashed(_)) {
                    if let Some(state) = handle.try_state::<AppState>() {
                        if state.sysproxy.status().enabled {
                            state.sysproxy.disable();
                            let _ = handle.emit("sysproxy://status", state.sysproxy.status());
                            refresh_tray(&handle);
                        }
                    }
                }
            }));

            // 状态一致性守护：上次会话若异常退出（journal 残留），先全量还原系统代理。
            #[cfg(target_os = "macos")]
            let backend = sysproxy::NetworkSetup;
            #[cfg(not(target_os = "macos"))]
            let backend = sysproxy::UnsupportedBackend;
            let sysproxy = SysProxyManager::new(backend, data_dir.join("sysproxy-journal.json"));
            if sysproxy.recover_stale() {
                eprintln!("recovered stale system proxy from previous session");
            }

            let handle = app.handle().clone();
            app.manage(AppState {
                store: Mutex::new(store),
                core_slot: Mutex::new(core),
                sysproxy,
                mode: Mutex::new("rule".to_string()),
                tray: Mutex::new(None),
                ws: WsHub::default(),
                app: handle,
                data_dir: data_dir.clone(),
            });

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
            // 关窗驻留托盘，不退出。
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
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
            set_mixed_port,
            set_allow_lan,
            autostart_status,
            autostart_set,
            core_version,
            core_binary_info,
            core_install,
            proxies_snapshot,
            select_proxy,
            test_group_delay,
            test_node_delay,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // 任何退出路径都先停 WS 桥、还原系统代理、收内核。
            if let tauri::RunEvent::Exit = event {
                if let Some(state) = app.try_state::<AppState>() {
                    state.ws.unsubscribe_all();
                    state.sysproxy.disable();
                    state.core().stop();
                }
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
