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

#[cfg(target_os = "macos")]
mod macos_icon;
mod coexist;
mod config_transition;
mod core_manager;
mod diagnose;
mod site_tests;
#[cfg(windows)]
mod windows_appearance;
mod log_files;
mod mihomo_api;
mod notifications;
mod process;
mod platform;
mod update_check;
mod scheduler;
mod sysproxy;
#[cfg(target_os = "macos")]
mod tray_speed;
mod tray_state;
#[cfg(target_os = "macos")]
mod tun;
#[cfg(windows)]
mod tun_windows;
#[cfg(windows)]
mod service_client;
#[cfg(target_os = "linux")]
mod linux_service;
#[cfg(target_os = "linux")]
mod tun_linux;
mod ws_bridge;

mod core_download;

use core_download::{resolve_core_binary, resolve_core_for, CoreBinaryInfo, InstallProgress};
use core_manager::{CoreManager, CoreStatus};
use sysproxy::{SysProxyManager, SysProxyStatus};
use ws_bridge::WsHub;

pub struct AppState {
    /// 轻量交接退出：RunEvent::Exit 时跳过清理（内核留给系统收养）。
    handoff: std::sync::atomic::AtomicBool,
    /// Explicit updater restart handoff; separate from the one-shot window guard.
    update_handoff: std::sync::atomic::AtomicBool,
    /// 安全模式：坏覆写导致内核启动失败后，跳过覆写链启动一次。
    safe_mode: std::sync::atomic::AtomicBool,
    config_transition: std::sync::atomic::AtomicBool,
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
    /// 托盘节点子菜单统一状态（此前 9 个分散锁的字段全部收拢于此）
    node_menu: Mutex<tray_state::TrayState>,
    /// 应用句柄：换核/重装后给新 CoreManager 重挂状态回调用。
    app: AppHandle,
    /// 更新设置（#28：自动检查/自动下载）。
    update: Mutex<crossbow_core::model::UpdateSettings>,
    /// 速率历史环形缓冲（#181）：托盘通道每秒喂入，与页面存亡无关；
    /// 首页挂载时一次取回预填「最近 120 秒」图表。
    traffic_history: Mutex<std::collections::VecDeque<TrafficSample>>,
}

/// macOS Tahoe 会按 NSWindow 外观绘制浅色外沿。同步窗口外观到应用主题，
/// 避免深色界面的系统描边仍按 macOS 浅色外观渲染。
#[cfg(target_os = "macos")]
fn apply_macos_window_appearance(win: &tauri::WebviewWindow, theme: &str) {
    let _ = win.set_background_color(Some(tauri::utils::config::Color(10, 12, 16, 255)));

    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
        NSView,
    };
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = win.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return;
    };
    // Tauri owns the NSView for the lifetime of this window; setup and window
    // recreation both invoke this helper on the AppKit main thread.
    unsafe {
        let view = &*(appkit.ns_view.as_ptr().cast::<NSView>());
        if let Some(ns_window) = view.window() {
            let name = match theme {
                "dark" => Some(NSAppearanceNameDarkAqua),
                "light" => Some(NSAppearanceNameAqua),
                _ => None,
            };
            if let Some(name) = name {
                let appearance = NSAppearance::appearanceNamed(name);
                ns_window.setAppearance(appearance.as_deref());
            } else {
                ns_window.setAppearance(None);
            }
        }
    }
}

/// Keep the OS-owned title bar and the webview on the same theme.
fn apply_window_appearance(win: &tauri::WebviewWindow, theme: &str) {
    #[cfg(windows)]
    {
        let native = match theme {
            "dark" => Some(tauri::Theme::Dark),
            "light" => Some(tauri::Theme::Light),
            _ => None,
        };
        let _ = win.set_theme(native);
    }
    #[cfg(target_os = "macos")]
    apply_macos_window_appearance(win, theme);
    let dark = match theme {
        "dark" => true,
        "light" => false,
        _ => win.theme().ok() == Some(tauri::Theme::Dark),
    };
    #[cfg(windows)]
    windows_appearance::apply(win, dark);
    let (r, g, b) = if dark { (10, 12, 16) } else { (246, 247, 249) };
    let _ = win.set_background_color(Some(tauri::utils::config::Color(r, g, b, 255)));
}

/// 活动订阅里 proxy-groups 的声明顺序（代理页排序与托盘主组探测共用）。
fn profile_group_order(handle: &AppHandle) -> Vec<String> {
    let Some(state) = handle.try_state::<AppState>() else {
        return Vec::new();
    };
    let content = {
        let store = state.store.lock().unwrap();
        store
            .data()
            .active_profile
            .clone()
            .and_then(|id| store.data().profile(&id).map(|p| p.content.clone()))
    };
    let Some(content) = content else { return Vec::new() };
    serde_yaml::from_str::<serde_yaml::Value>(&content)
        .ok()
        .and_then(|v| {
            v.get("proxy-groups")
                .and_then(|g| g.as_sequence())
                .map(|seq| {
                    seq.iter()
                        .filter_map(|g| {
                            g.get("name").and_then(|n| n.as_str()).map(String::from)
                        })
                        .collect()
                })
        })
        .unwrap_or_default()
}

/// Native liveness check also recognizes exited Windows processes with retained handles.
fn pid_alive(pid: u32) -> bool { core_manager::pid_is_alive(pid) }

/// 单秒速率采样（字节/秒）。
#[derive(serde::Serialize, Clone, Copy)]
pub struct TrafficSample {
    pub up: u64,
    pub down: u64,
}

/// 速率历史环形缓冲容量 = 前端图表窗口（最近 120 秒）。
const TRAFFIC_HISTORY_LEN: usize = 120;

impl AppState {
    /// 当前内核管理器快照（clone 便宜：内部是 Arc）。
    pub fn core(&self) -> CoreManager {
        self.core_slot.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// 追加一秒采样，超出窗口即淘汰最旧（#181）
    pub fn traffic_history_push(&self, up: u64, down: u64) {
        let mut ring = self.traffic_history.lock().unwrap_or_else(|e| e.into_inner());
        ring.push_back(TrafficSample { up, down });
        while ring.len() > TRAFFIC_HISTORY_LEN {
            ring.pop_front();
        }
    }

    /// 快照（旧 → 新）。内核停止时清空，图表诚实回到空态。
    pub fn traffic_history_snapshot(&self) -> Vec<TrafficSample> {
        self.traffic_history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .copied()
            .collect()
    }

    pub fn traffic_history_clear(&self) {
        self.traffic_history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
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
            platform::validate_config(store.data().engine.engine, &r.config)?;
            // 不在这里落盘。配置切换要等内核就绪后才保存；这里保存会把
            // 尚未确认的内存修改写进磁盘。覆写错误的清除留在内存里，
            // 下一次成功的保存会带上。
            store.data_mut().clear_override_errors();
            Ok(r)
        }
        Err(crossbow_core::RenderError::OverrideFailed { id, detail, .. }) => {
            store
                .data_mut()
                .set_override_error(&id, Some(detail.clone()));
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

/// A successful spawn only means the process exists. Do not expose TUN or
/// system proxy as enabled until the controller confirms the core is ready.
fn wait_core_running(state: &AppState) -> Result<(), String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        match state.core().status() {
            CoreStatus::Running => return Ok(()),
            CoreStatus::Crashed(error) => return Err(error),
            CoreStatus::Stopped => return Err("内核已停止".into()),
            CoreStatus::Starting => {}
        }
        if std::time::Instant::now() >= deadline {
            return Err("等待内核启动超时".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// 调用方必须已持有 core_op（start_core / restart_core 入口）。
pub(crate) fn start_core_locked(state: &AppState) -> Result<(), String> {
    let safe = state.safe_mode.load(std::sync::atomic::Ordering::SeqCst);
    let is_singbox = {
        let store = state.store.lock().unwrap();
        store.data().engine.engine == crossbow_core::Engine::SingBox
    };
    let need = if is_singbox { "sing-box" } else { "mihomo" };
    let mut binary = ensure_engine_binary(state, need)?;
    let rendered = render_current(state, !safe)?;
    // CoreManager::start synchronously notifies the status callback, which also
    // reads the store. Release this guard before either TUN or regular startup.
    let rt = {
        let store = state.store.lock().unwrap();
        let tun_enable = store.data().engine.tun_enable;
        // v6 双栈注入与原生 v6 可达性联动（#136）：v4-only 网络上 DIRECT
        // 的 v6 目的地发不出去，只会在日志里刷 no route to host。
        #[cfg(target_os = "macos")]
        let native_ipv6 = !tun_enable || crate::tun::has_native_ipv6();
        #[cfg(not(target_os = "macos"))]
        let native_ipv6 = true;
        RuntimeConfig {
            engine: store.data().engine.engine,
            mixed_port: store.data().engine.mixed_port,
            allow_lan: store.data().engine.allow_lan,
            tun_enable,
            native_ipv6,
            ..RuntimeConfig::default()
        }
    };
    // TUN 模式：内核需 root 权限创建 utun。用 osascript 管理员授权把内核
    // 二进制 setuid root（一次性），再由应用正常拉起（子进程继承 root）。
    if rt.tun_enable {
        #[cfg(target_os = "macos")]
        {
            binary = crate::tun::ensure_root_binary(state, &binary)?;
            if binary != state.core().binary_path() {
                if !state.core().stop() {
                    return Err("旧内核未退出，已中止 TUN 内核切换".into());
                }
                let rooted_core = CoreManager::new(binary, state.data_dir.join("runtime"));
                rooted_core.set_callback(core_status_callback(state.app.clone()));
                *state.core_slot.lock().unwrap_or_else(|e| e.into_inner()) = rooted_core;
            }
            // Keep Crossbow's own system proxy active; only a third-party
            // proxy would divert proxy-aware apps away from this TUN core.
            if !state.sysproxy.status().enabled {
                let _ = state.sysproxy.suspend_for_tun()?;
            }
            return state.core().start(&rendered.config, &rt);
        }
        #[cfg(target_os = "windows")]
        {
            // Windows TUN（#81）：内核须以 SYSTEM 运行创建 wintun 适配器，
            // GUI 无从提权——把「拉起进程」外包给特权服务（crossbow-service，
            // NSIS 注册开机自启），端口/密钥/配置注入/就绪探测仍由
            // CoreManager::start_with_spawner 统一簿记。
            // 服务只运行钉定的安装目录副本：引擎选中的是数据目录下载内核
            // 时（升级后常见），TUN 会话改用钉定副本，这里记录差异供诊断
            state.core().log_decision(&format!(
                "tun: engine core {} → service pinned sibling",
                binary.display()
            ));
            let spawner = crate::service_client::make_core_spawner(&binary)?;
            if !state.sysproxy.status().enabled {
                let _ = state.sysproxy.suspend_for_tun()?;
            }
            return state.core().start_with_spawner(&rendered.config, &rt, Box::new(spawner));
        }
        #[cfg(target_os = "linux")]
        {
            if is_singbox {
                return Err("Linux TUN 当前仅支持 mihomo".into());
            }
            crossbow_service::linux_network::ensure_available(None, &[])?;
            let spawner = linux_service::make_core_spawner()?;
            return state.core().start_with_spawner(&rendered.config, &rt, Box::new(spawner));
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            return Err("当前平台暂不支持 TUN，请使用显式 HTTP/SOCKS 代理".into());
        }
    }
    #[cfg(target_os = "macos")]
    {
        ensure_unprivileged_core_binary(state, need, binary)?;
    }
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
    if cur.exists() && core_download::is_core_bin(&cur, need) {
        return Ok(cur);
    }
    let Some(p) = core_download::resolve_core_for(&state.data_dir, Some(need)) else {
        return Err(format!(
            "内核「{need}」未安装 — 到设置页下载对应内核后再启动"
        ));
    };
    if !state.core().stop() {
        return Err("旧内核未退出，已中止内核切换".into());
    }
    let new_core = CoreManager::new(p.clone(), state.data_dir.join("runtime"));
    // 换核 = 新管理器：必须重挂状态回调（托盘速率/前端事件/崩溃守卫都靠它）
    new_core.set_callback(core_status_callback(state.app.clone()));
    *state.core_slot.lock().unwrap_or_else(|e| e.into_inner()) = new_core;
    state
        .core()
        .log_decision(&format!("ensure: swapped to {}", p.display()));
    Ok(p)
}

/// TUN 授权副本长期保留 setuid，但 TUN 关闭后切回普通内核副本，
/// 避免普通代理运行时也以 root 身份启动。
#[cfg(target_os = "macos")]
fn ensure_unprivileged_core_binary(
    state: &AppState,
    need: &str,
    current: PathBuf,
) -> Result<PathBuf, String> {
    let tun_dir = state.data_dir.join("tun-binaries");
    if !current.starts_with(&tun_dir) {
        return Ok(current);
    }
    let Some(binary) = core_download::resolve_core_for(&state.data_dir, Some(need)) else {
        return Err(format!("内核「{need}」未安装 — 到设置页下载对应内核后再启动"));
    };
    if !state.core().stop() {
        return Err("TUN 内核未退出，已中止普通内核启动".into());
    }
    let new_core = CoreManager::new(binary.clone(), state.data_dir.join("runtime"));
    new_core.set_callback(core_status_callback(state.app.clone()));
    *state.core_slot.lock().unwrap_or_else(|e| e.into_inner()) = new_core;
    Ok(binary)
}

/// 改存储里的配置，并让正在跑的内核跟上。
///
/// 先记住改之前的存储和运行配置，再在内存里改。渲染失败、热重载失败、
/// 重启失败，都把内存和磁盘退回改之前，并尽量把旧内核拉回来。新状态只有
/// 在内核确认就绪，或内核本来没在跑时，才落盘。
///
/// TUN 开关不走这里。它要换特权进程，仍由各平台 `toggle_tun` 处理。
fn commit_config_change(
    state: &AppState,
    mutate: impl FnOnce(&mut crossbow_core::StoreSchema) -> Result<(), String>,
) -> Result<(), String> {
    commit_config_change_inner(state, mutate, false)
}

fn commit_config_change_inner(
    state: &AppState,
    mutate: impl FnOnce(&mut crossbow_core::StoreSchema) -> Result<(), String>,
    exit_safe_mode: bool,
) -> Result<(), String> {
    let _op = state.core_op.lock().unwrap_or_else(|e| e.into_inner());
    let _recovery = config_transition::RecoveryOwner::enter(&state.config_transition);
    let before = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .clone();
    let previous_safe = state.safe_mode.load(std::sync::atomic::Ordering::SeqCst);
    let proxy = state.sysproxy.status();
    {
        let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(error) = mutate(store.data_mut()) {
            *store.data_mut() = before;
            return Err(error);
        }
        if store.data().engine.tun_enable != before.engine.tun_enable {
            *store.data_mut() = before;
            return Err("TUN 开关不走配置切换".into());
        }
    }
    if exit_safe_mode {
        state
            .safe_mode
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
    let after = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .clone();
    let old_config = state.core().current_config();
    let status = state.core().status();
    let running = matches!(status, CoreStatus::Running | CoreStatus::Starting);
    let live = config_transition::LiveCore {
        running,
        mihomo: before.engine.engine == crossbow_core::Engine::Mihomo
            && status == CoreStatus::Running,
        controller: state.core().controller().is_some(),
        cached_config: old_config.is_some(),
        service_managed: cfg!(any(windows, target_os = "linux")) && before.engine.tun_enable,
    };
    let plan = if config_transition::needs_new_process(
        before.engine.engine,
        after.engine.engine,
        before.engine.mixed_port,
        after.engine.mixed_port,
        before.engine.allow_lan,
        after.engine.allow_lan,
        before.engine.tun_enable,
        after.engine.tun_enable,
    ) {
        config_transition::plan_identity(live)
    } else {
        config_transition::plan_content(live)
    };
    state
        .core()
        .log_decision(&format!("config change: {plan:?}"));
    let mut transaction = ConfigChange {
        state,
        before,
        previous_safe,
        proxy,
        old_config,
        plan,
        touched_core: false,
        restarted: false,
        stopped: false,
    };
    let result = config_transition::execute(&mut transaction);
    // Rollback may have changed the actual proxy state, even when it failed.
    let _ = state.app.emit("sysproxy://status", state.sysproxy.status());
    if result.is_ok() && running {
        // A successful reload does not produce a CoreManager Running callback.
        // Invalidate the old group's cache before fetching the new group order.
        state
            .node_menu
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .invalidate_config();
        #[cfg(target_os = "macos")]
        refresh_tray_nodes(&state.app, false);
    }
    let _ = state.app.emit("core://status", state.core().status());
    sync_tray_traffic(&state.app);
    update_logs_persistence(&state.app);
    refresh_tray(&state.app);
    result
}

struct ConfigChange<'a> {
    state: &'a AppState,
    before: crossbow_core::StoreSchema,
    previous_safe: bool,
    proxy: SysProxyStatus,
    old_config: Option<String>,
    plan: config_transition::Plan,
    touched_core: bool,
    restarted: bool,
    stopped: bool,
}

impl ConfigChange<'_> {
    fn restart(&mut self) -> Result<(), String> {
        self.restarted = true;
        if !self.state.core().stop() {
            return Err("旧内核未退出，已中止切换".into());
        }
        self.stopped = true;
        self.touched_core = true;
        start_core_locked(self.state).and_then(|()| wait_core_running(self.state))
    }

    fn restore_runtime(&mut self) -> Result<(), String> {
        if self.restarted && !self.stopped {
            return Err("无法确认旧内核已停止，保留现场供重试".into());
        }
        if !self.touched_core {
            return Ok(());
        }
        let mut reload_error = None;
        if !self.restarted {
            if let Some(old) = &self.old_config {
                match reload_final_config(self.state, old) {
                    Ok(()) => return Ok(()),
                    Err(error) => reload_error = Some(error),
                }
            }
        }
        // Stop failed candidates and their restart watchers before restoring.
        self.restart().map_err(|error| match reload_error {
            Some(reload) => format!("旧配置重载失败：{reload}；重启失败：{error}"),
            None => error,
        })
    }
}

impl config_transition::Transaction for ConfigChange<'_> {
    fn apply(&mut self) -> Result<(), String> {
        if self.plan == config_transition::Plan::PersistOnly {
            return Ok(());
        }
        // Render/validate before stopping a healthy core. A broken override must
        // not interrupt the current network merely to discover it cannot render.
        let rendered = render_current(
            self.state,
            !self
                .state
                .safe_mode
                .load(std::sync::atomic::Ordering::SeqCst),
        )?;
        let controller = self.state.core().controller();
        let (port, secret) = controller.unwrap_or((0, String::new()));
        let final_config =
            crossbow_core::apply_runtime(&rendered.config, &runtime_for(self.state, port, secret))?;
        if self.plan == config_transition::Plan::HotReload {
            self.touched_core = true;
            if let Err(error) = reload_final_config(self.state, &final_config) {
                self.state
                    .core()
                    .log_decision(&format!("hot reload failed, restarting: {error}"));
                self.restart()
                    .map_err(|restart| format!("热重载失败：{error}；重启失败：{restart}"))?;
            }
        } else {
            self.restart()?;
        }
        // Proxy retargeting is part of the transaction, before persistence.
        if self.proxy.enabled {
            let store = self.state.store.lock().unwrap_or_else(|e| e.into_inner());
            let port = store.data().engine.mixed_port;
            let tun = store.data().engine.tun_enable;
            drop(store);
            restore_config_proxy(
                self.state,
                SysProxyStatus {
                    enabled: true,
                    port,
                },
                tun,
            )?;
        }
        Ok(())
    }

    fn persist(&mut self) -> Result<(), String> {
        persist_schema(self.state)
    }

    fn rollback(&mut self) -> Result<(), String> {
        self.state
            .safe_mode
            .store(self.previous_safe, std::sync::atomic::Ordering::SeqCst);
        let mut errors = Vec::new();
        // Disk failure must not prevent restoring the old runtime/network.
        if let Err(error) = restore_store(&self.state.store, &self.before) {
            errors.push(format!("旧状态保存失败：{error}"));
        }
        let runtime_ok = match self.restore_runtime() {
            Ok(()) => true,
            Err(error) => {
                errors.push(format!("旧内核恢复失败：{error}"));
                false
            }
        };
        if runtime_ok {
            if let Err(error) =
                restore_config_proxy(self.state, self.proxy, self.before.engine.tun_enable)
            {
                errors.push(format!("系统代理恢复失败：{error}"));
            }
        } else if self.stopped && self.state.core().stop() {
            // Only restore third-party TUN proxy settings once our routes are gone.
            if let Err(error) = recover_config_network(self.state) {
                errors.push(error);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("；"))
        }
    }
}

fn reload_final_config(state: &AppState, config: &str) -> Result<(), String> {
    let (port, secret) = state
        .core()
        .controller()
        .ok_or("控制器不可用，无法热重载")?;
    let core = state.core();
    core.write_config_file(config)?;
    mihomo_api::Controller { port, secret }.reload_config(&core.config_path())?;
    core.replace_cached_config(config.to_string());
    Ok(())
}

fn restore_config_proxy(
    state: &AppState,
    expected: SysProxyStatus,
    tun: bool,
) -> Result<(), String> {
    let actual = state.sysproxy.status();
    if actual.enabled == expected.enabled && (!expected.enabled || actual.port == expected.port) {
        return Ok(());
    }
    if expected.enabled {
        if tun {
            state.sysproxy.enable_during_tun(expected.port)
        } else {
            state.sysproxy.enable(expected.port)
        }
    } else if tun {
        state.sysproxy.disable_during_tun()
    } else {
        state.sysproxy.disable()
    }
}

fn runtime_for(state: &AppState, controller_port: u16, controller_secret: String) -> RuntimeConfig {
    let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
    let tun_enable = store.data().engine.tun_enable;
    #[cfg(target_os = "macos")]
    let native_ipv6 = !tun_enable || crate::tun::has_native_ipv6();
    #[cfg(not(target_os = "macos"))]
    let native_ipv6 = true;
    RuntimeConfig {
        engine: store.data().engine.engine,
        mixed_port: store.data().engine.mixed_port,
        allow_lan: store.data().engine.allow_lan,
        controller_port,
        controller_secret,
        log_level: "info".into(),
        tun_enable,
        native_ipv6,
    }
}

fn persist_schema(state: &AppState) -> Result<(), String> {
    let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
    store.save().map_err(|e| e.to_string())
}

/// Restore in-memory state before attempting I/O; a failed save must not
/// leave the rejected candidate visible to the next operation.
fn restore_store(store: &Mutex<Store>, before: &crossbow_core::StoreSchema) -> Result<(), String> {
    let mut store = store.lock().unwrap_or_else(|e| e.into_inner());
    *store.data_mut() = before.clone();
    store.save().map_err(|e| e.to_string())
}

/// 内核没回来时，按退出路径把系统代理还回去，避免代理指向一个死端口。
fn recover_network_if_core_dead(state: &AppState) {
    if let Err(error) = recover_config_network(state) {
        eprintln!("network recovery failed: {error}");
    }
}

fn recover_config_network(state: &AppState) -> Result<(), String> {
    let tun = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .engine
        .tun_enable;
    let proxy = state.sysproxy.disable();
    let tun_proxy = if tun {
        state.sysproxy.recover_tun_without_core()
    } else {
        Ok(())
    };
    match (proxy, tun_proxy) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(a), Err(b)) => Err(format!("系统代理恢复失败：{a}；TUN 代理恢复失败：{b}")),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
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
async fn import_profile_url(
    app: AppHandle,
    state: State<'_, AppState>,
    url: String,
    name: Option<String>,
) -> Result<Profile, String> {
    let profile = import_url_blocking(&state, &url, name.as_deref())?;
    refresh_tray(&app);
    Ok(profile)
}

/// 导入本地/粘贴的配置内容。
#[tauri::command]
async fn import_profile_content(
    app: AppHandle,
    state: State<'_, AppState>,
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
async fn import_profile_file(
    app: AppHandle,
    state: State<'_, AppState>,
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

/// 刷新一次订阅（命令与调度器共用）：拉取 → 校验 → 当前档案走配置切换
/// （能热重载就热重载，失败退回旧内容）→ 记录结果。网络请求不持有 store 锁。
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

    let active = {
        let store = state.store.lock().unwrap();
        store.data().active_profile.as_deref() == Some(id)
    };
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

/// 刷新订阅并落库；正在运行则热重启应用新配置。
#[tauri::command]
async fn update_profile(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<Profile, String> {
    let profile = refresh_profile_blocking(&state, &id)?;
    notifications::resolve(&app, &format!("profile-auto-update-{id}"));
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
    commit_config_change_inner(&state, |_| Ok(()), true)
}

#[tauri::command]
fn update_override(
    state: State<AppState>,
    id: String,
    content: Option<String>,
    name: Option<String>,
) -> Result<(), String> {
    let affects_running = content.is_some() && override_affects_active(&state, &id);
    let edit = |data: &mut crossbow_core::StoreSchema| {
        let o = data
            .overrides
            .iter_mut()
            .find(|o| o.id == id)
            .ok_or("override not found")?;
        if let Some(c) = content {
            o.content = c;
        }
        if let Some(n) = name {
            o.name = n;
        }
        Ok(())
    };
    if affects_running {
        commit_config_change(&state, edit)?;
    } else {
        let mut store = state.store.lock().unwrap();
        edit(store.data_mut())?;
        store.save().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 这条覆写是否挂在当前档案上。没挂上的修改不该碰正在跑的内核。
fn override_affects_active(state: &AppState, override_id: &str) -> bool {
    let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
    store
        .data()
        .active_profile
        .as_deref()
        .and_then(|pid| store.data().profile(pid))
        .is_some_and(|profile| profile.override_ids.iter().any(|id| id == override_id))
}

/// 删除覆写（并从所有 Profile 摘除引用）。
#[tauri::command]
async fn remove_override(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let affects_running = override_affects_active(&state, &id);
    let edit = |data: &mut crossbow_core::StoreSchema| {
        data.overrides.retain(|o| o.id != id);
        for profile in data.profiles.iter_mut() {
            profile.override_ids.retain(|oid| oid != &id);
        }
        Ok(())
    };
    if affects_running {
        commit_config_change(&state, edit)?;
    } else {
        let mut store = state.store.lock().unwrap();
        edit(store.data_mut())?;
        store.save().map_err(|e| e.to_string())?;
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
    let affects_running = {
        let store = state.store.lock().unwrap();
        store.data().active_profile.as_deref() == Some(profile_id.as_str())
    };
    let edit = |data: &mut crossbow_core::StoreSchema| {
        let profile = data
            .profiles
            .iter_mut()
            .find(|p| p.id == profile_id)
            .ok_or("profile not found")?;
        if enabled {
            if !profile.override_ids.contains(&override_id) {
                profile.override_ids.push(override_id);
            }
        } else {
            profile.override_ids.retain(|oid| oid != &override_id);
        }
        Ok(())
    };
    if affects_running {
        commit_config_change(&state, edit)?;
    } else {
        let mut store = state.store.lock().unwrap();
        edit(store.data_mut())?;
        store.save().map_err(|e| e.to_string())?;
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
    let affects_running = override_affects_active(&state, &id);
    let edit = |data: &mut crossbow_core::StoreSchema| {
        let override_def = data
            .overrides
            .iter_mut()
            .find(|o| o.id == id)
            .ok_or("override not found")?;
        override_def.enabled = enabled;
        Ok(())
    };
    if affects_running {
        commit_config_change(&state, edit)?;
    } else {
        let mut store = state.store.lock().unwrap();
        edit(store.data_mut())?;
        store.save().map_err(|e| e.to_string())?;
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
    // 新引擎写进存储的时机在内核确认就绪之后。失败会退回旧引擎并拉起旧核。
    commit_config_change(&state, |data| {
        data.engine.engine = target;
        Ok(())
    })?;
    state.core().log_decision(&format!(
        "set_engine: -> {engine} status={:?}",
        state.core().status()
    ));
    refresh_tray(&app);
    // 通知侧栏 footer 等跟随引擎的 UI 重新取标识
    let _ = app.emit("engine://changed", engine.clone());
    Ok(())
}

/// TUN 开关（macOS）；系统代理有独立开关，可共用同一个内核。
#[tauri::command]
async fn set_tun(app: AppHandle, state: State<'_, AppState>, enable: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        notifications::publish(&app, notifications::Notice::new(notifications::Kind::Progress, "tray.tun", "tun-switch"));
        let result = crate::tun::toggle_tun(&state, enable);
        notifications::result(&app, "tun-switch", "tray.tun", result.clone());
        let actual = state.store.lock().unwrap().data().engine.tun_enable;
        let _ = app.emit("tun://status", actual);
        let _ = app.emit("sysproxy://status", state.sysproxy.status());
        refresh_tray(&app);
        result
    }
    #[cfg(target_os = "windows")]
    {
        notifications::publish(&app, notifications::Notice::new(notifications::Kind::Progress, "tray.tun", "tun-switch"));
        let result = crate::tun_windows::toggle_tun(&state, enable);
        notifications::result(&app, "tun-switch", "tray.tun", result.clone());
        let actual = state.store.lock().unwrap().data().engine.tun_enable;
        let _ = app.emit("tun://status", actual);
        let _ = app.emit("sysproxy://status", state.sysproxy.status());
        refresh_tray(&app);
        result
    }
    #[cfg(target_os = "linux")]
    {
        let result = crate::tun_linux::toggle_tun(&state, enable);
        notifications::result(&app, "tun-switch", "tray.tun", result.clone());
        let actual = linux_service::status().map(|s| s.is_some_and(|s| s.active))
            .unwrap_or_else(|_| state.store.lock().unwrap().data().engine.tun_enable);
        let _ = app.emit("tun://status", actual);
        refresh_tray(&app);
        result
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = enable;
        let result = Err("当前平台暂不支持 TUN，请使用显式 HTTP/SOCKS 代理".into());
        notifications::result(&app, "tun-switch", "tray.tun", result.clone());
        result
    }
}

#[derive(serde::Serialize)]
struct TunStatus {
    enabled: bool,
    /// 内核二进制已 setuid root（可无感开 TUN）
    ready: bool,
    core_running: bool,
}

#[tauri::command]
fn tun_status(state: State<AppState>) -> TunStatus {
    #[cfg(target_os = "macos")]
    {
        let enabled = {
            let store = state.store.lock().unwrap();
            store.data().engine.tun_enable
        };
        let is_singbox = {
            let store = state.store.lock().unwrap();
            store.data().engine.engine == crossbow_core::Engine::SingBox
        };
        let need = if is_singbox { "sing-box" } else { "mihomo" };
        // 授权副本在 tun-binaries/<版本>/ 下（PR13），检查它的 setuid 位
        let ready = crate::tun::authorized_copy_exists(&state.data_dir, need);
        TunStatus {
            enabled,
            ready,
            core_running: state.core().status() == CoreStatus::Running,
        }
    }
    #[cfg(target_os = "windows")]
    {
        let enabled = {
            let store = state.store.lock().unwrap();
            store.data().engine.tun_enable
        };
        TunStatus {
            enabled,
            // 服务随安装器注册即视为就绪（GUI 无需再授权）
            ready: crate::service_client::installed(),
            core_running: state.core().status() == CoreStatus::Running,
        }
    }
    #[cfg(target_os = "linux")]
    {
        TunStatus {
            enabled: linux_service::status().map(|s| s.is_some_and(|s| s.active))
                .unwrap_or_else(|_| state.store.lock().unwrap().data().engine.tun_enable),
            ready: linux_service::installed(),
            core_running: state.core().status() == CoreStatus::Running,
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        TunStatus {
            enabled: false,
            ready: false,
            core_running: false,
        }
    }
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
async fn set_active_profile(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    commit_config_change(&state, |data| data.set_active_profile(&id))?;
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
fn platform_capabilities() -> platform::Capabilities {
    platform::capabilities()
}

#[tauri::command]
async fn core_start(state: State<'_, AppState>) -> Result<(), String> {
    start_core(&state)
}

#[tauri::command]
async fn core_stop(state: State<'_, AppState>) -> Result<(), String> {
    let _operation = state.core_op.lock().unwrap_or_else(|e| e.into_inner());
    if state.core().stop() {
        Ok(())
    } else {
        Err("内核未退出，请重试停止".into())
    }
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
    state.ws.want_channel("traffic");
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
    state.ws.want_channel("connections");
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
    state.ws.want_channel("logs");
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
async fn run_diagnosis(state: State<'_, AppState>) -> Result<Vec<diagnose::CheckResult>, String> {
    Ok(diagnose::run(&state))
}

#[tauri::command]
fn current_outbound(state: State<AppState>) -> Result<mihomo_api::OutboundInfo, String> {
    let fallback = state
        .core()
        .current_config()
        .and_then(|c| extract_final_outbound(&c));
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
    notifications::publish(&app, notifications::Notice::new(notifications::Kind::Progress, "core.install", "core-download"));
    let handle = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || core_install_inner(handle, engine)).await.map_err(|e| e.to_string()).and_then(|result| result);
    notifications::result(&app, "core-download", if result.is_ok() { "core.installReady" } else { "core.install" }, result.as_ref().map(|_| ()).map_err(Clone::clone));
    result
}
fn core_install_inner(app: AppHandle, engine: Option<String>) -> Result<CoreBinaryInfo, String> {
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let kind = match engine.as_deref() {
        Some("singbox") => core_download::CoreKind::SingBox,
        _ => core_download::CoreKind::Mihomo,
    };
    let h = app.clone();
    let info = core_download::install_core(&data_dir, kind, &move |p: InstallProgress| {
        let mut notice = notifications::Notice::new(notifications::Kind::Progress, "core.install", "core-download");
        notice.description = p.stage.clone();
        notifications::publish(&h, notice);
        let _ = h.emit("core://install-progress", p);
    })?;
    // 安装后重新装载 CoreManager 的二进制路径：直接替换状态里的管理器。
    let state = app.state::<AppState>();
    // 与启动链互斥：换核期间不允许并发的 start_core 拿到半新半旧的槽位
    let _op = state.core_op.lock().unwrap_or_else(|e| e.into_inner());
    let new_core = CoreManager::new(info.clone(), data_dir.join("runtime"));
    new_core.set_callback(core_status_callback(app.clone()));
    state.core().stop();
    *state.core_slot.lock().unwrap_or_else(|e| e.into_inner()) = new_core;
    Ok(CoreBinaryInfo {
        path: info.display().to_string(),
        source: "data".into(),
        version: core_version_of(&info).unwrap_or_default(),
    })
}

/// 内核版本（执行 `<bin> -v` 首行）；sing-box 只认 `version` 子命令，依次尝试。
fn core_version_of(bin: &PathBuf) -> Option<String> {
    for flag in ["-v", "version"] {
        let Ok(out) = crate::process::command(bin).arg(flag).output() else {
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

/// 同 profile_group_order，但直接吃配置文本。
pub fn profile_group_order_str(content: &str) -> Vec<String> {
    serde_yaml::from_str::<serde_yaml::Value>(content)
        .ok()
        .and_then(|v| {
            v.get("proxy-groups")
                .and_then(|g| g.as_sequence())
                .map(|seq| {
                    seq.iter()
                        .filter_map(|g| {
                            g.get("name").and_then(|n| n.as_str()).map(String::from)
                        })
                        .collect()
                })
        })
        .unwrap_or_default()
}

#[tauri::command]
fn proxies_snapshot(state: State<AppState>) -> Result<Vec<mihomo_api::GroupView>, String> {
    let mut groups = controller_client(&state)?.groups()?;
    // 按订阅声明顺序展示（与 ClashX 一致，主选择组在前）；GLOBAL 恒最后
    let order = {
        let store = state.store.lock().unwrap();
        store
            .data()
            .active_profile
            .clone()
            .and_then(|id| {
                store
                    .data()
                    .profile(&id)
                    .map(|p| crate::profile_group_order_str(&p.content))
            })
            .unwrap_or_default()
    };
    groups.sort_by(|a, b| {
        let ga = (a.name == "GLOBAL") as u8;
        let gb = (b.name == "GLOBAL") as u8;
        let pa = order
            .iter()
            .position(|n| n == &a.name)
            .unwrap_or(usize::MAX);
        let pb = order
            .iter()
            .position(|n| n == &b.name)
            .unwrap_or(usize::MAX);
        ga.cmp(&gb).then(pa.cmp(&pb))
    });
    Ok(groups)
}

#[tauri::command]
async fn select_proxy(state: State<'_, AppState>, group: String, name: String) -> Result<(), String> {
    controller_client(&state)?.select_proxy(&group, &name)
}

#[tauri::command]
async fn test_group_delay(
    state: State<'_, AppState>,
    group: String,
) -> Result<BTreeMap<String, u64>, String> {
    controller_client(&state)?.test_group_delay(&group)
}

#[tauri::command]
async fn test_node_delay(state: State<'_, AppState>, node: String) -> Result<u64, String> {
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
        .map(|p| core_download::is_core_bin(&p, need))
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
    if let Some(win) = app.get_webview_window("main") {
        apply_window_appearance(&win, &theme);
    }
    let _ = app.emit("ui://theme", theme);
    Ok(())
}

/// 日志保存设置（#29）。
#[tauri::command]
fn logs_settings(state: State<AppState>) -> crossbow_core::model::LogSettings {
    state
        .store
        .lock()
        .unwrap()
        .data()
        .logs
        .clone()
}

#[tauri::command]
fn set_logs_save(app: AppHandle, state: State<AppState>, enabled: bool) -> Result<(), String> {
    {
        let mut store = state
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        store.data_mut().logs.save_enabled = enabled;
        store.save().map_err(|e| e.to_string())?;
    }
    update_logs_persistence(&app);
    Ok(())
}

#[tauri::command]
fn set_logs_retention(state: State<AppState>, days: u32) -> Result<(), String> {
    if ![1, 3, 5, 7, 15, 30].contains(&days) {
        return Err(format!("invalid retention days: {days}"));
    }
    {
        let mut store = state
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        store.data_mut().logs.retention_days = days;
        store.save().map_err(|e| e.to_string())?;
    }
    // 立即按新保留期清理一次
    let dir = log_files::logs_dir(&state.data_dir);
    log_files::sweep(&dir, days);
    Ok(())
}

// Network probes run off the GUI thread with bounded timeouts.
#[tauri::command]
async fn test_site(app: AppHandle, id: String) -> Result<site_tests::SiteResult, String> {
    let (port, running) = {
        let state = app.state::<AppState>();
        let port = state.store.lock().unwrap_or_else(|e| e.into_inner()).data().engine.mixed_port;
        (port, state.core().status() == CoreStatus::Running)
    };
    tauri::async_runtime::spawn_blocking(move || site_tests::probe(&id, port, running))
        .await.map_err(|e| e.to_string())?
}

// 必须异步 + spawn_blocking：detect 内部跑 ps/netstat 子进程（秒级），
// 同步命令会在主线程执行直接卡死整个 UI（实测 hang 1.07s 报告）。
#[tauri::command]
async fn coexist_detect(app: AppHandle) -> Vec<coexist::CoexistFinding> {
    tauri::async_runtime::spawn_blocking(move || coexist::detect(&app.state::<AppState>()))
        .await
        .unwrap_or_default()
}

#[tauri::command]
fn update_state_get(app: AppHandle) -> Option<update_check::UpdateInfo> {
    update_check::current(&app)
}

#[tauri::command]
async fn update_check_now(app: AppHandle) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (_, auto_download) = update_check::update_settings(&app);
        update_check::check_once(&app, auto_download, false)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn update_start_download(app: AppHandle) -> Result<(), String> {
    std::thread::spawn(move || {
        if let Err(e) = update_check::download_and_prepare(&app) {
            eprintln!("update download: {}", notifications::redact(&e));
        }
    });
    Ok(())
}

#[tauri::command]
fn update_install(app: AppHandle) -> Result<(), String> {
    update_check::install(&app)
}

#[tauri::command]
fn update_settings_get(state: State<AppState>) -> crossbow_core::model::UpdateSettings {
    state
        .store
        .lock()
        .unwrap()
        .data()
        .update
        .clone()
}

#[tauri::command]
fn set_update_auto_check(state: State<AppState>, enabled: bool) -> Result<(), String> {
    {
        let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store.data_mut().update.auto_check = enabled;
        store.save().map_err(|e| e.to_string())?;
    }
    let mut u = state.update.lock().unwrap_or_else(|e| e.into_inner());
    u.auto_check = enabled;
    Ok(())
}

#[tauri::command]
fn set_update_auto_download(state: State<AppState>, enabled: bool) -> Result<(), String> {
    {
        let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store.data_mut().update.auto_download = enabled;
        store.save().map_err(|e| e.to_string())?;
    }
    let mut u = state.update.lock().unwrap_or_else(|e| e.into_inner());
    u.auto_download = enabled;
    Ok(())
}

#[tauri::command]
fn open_logs_dir(state: State<AppState>) -> Result<(), String> {
    let dir = log_files::logs_dir(&state.data_dir);
    let _ = std::fs::create_dir_all(&dir);
    #[cfg(target_os = "macos")]
    {
        crate::process::command("open")
            .arg(&dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(windows)]
    {
        crate::process::command("explorer")
            .arg(&dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
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
async fn set_mixed_port(
    app: AppHandle,
    state: State<'_, AppState>,
    port: u16,
) -> Result<(), String> {
    if port < 1024 {
        return Err("端口不能小于 1024".into());
    }
    commit_config_change(&state, |data| {
        data.engine.mixed_port = port;
        Ok(())
    })?;
    let _ = app.emit("sysproxy://status", state.sysproxy.status());
    Ok(())
}

#[tauri::command]
async fn set_allow_lan(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    commit_config_change(&state, |data| {
        data.engine.allow_lan = enabled;
        Ok(())
    })?;
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
    let out = crate::process::command(&bin)
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
                let mut notice = notifications::Notice::new(notifications::Kind::Success, "import.done", "deep-link-import");
                notice.description = notifications::redact(&target);
                notifications::publish(&app, notice);
                refresh_tray(&app);
            }
            Err(e) => {
                notifications::result(&app, "deep-link-import", "import.failed", Err(format!("{target}: {e}")));
            }
        }
    });
}

/// 首页速率图预填（#181）：返回最近 120 秒采样（旧 → 新）
#[derive(serde::Serialize)]
struct TrafficHistoryEntry {
    up: u64,
    down: u64,
}

#[tauri::command]
fn traffic_history(state: State<AppState>) -> Vec<TrafficHistoryEntry> {
    state
        .traffic_history_snapshot()
        .into_iter()
        .map(|sample| TrafficHistoryEntry {
            up: sample.up,
            down: sample.down,
        })
        .collect()
}

#[tauri::command]
fn sysproxy_status(state: State<AppState>) -> SysProxyStatus {
    state.sysproxy.status()
}

/// 总开关：开启 = 确保内核运行 + 设置系统代理；关闭 = 还原系统代理（内核保持）。
fn toggle_sysproxy_state(state: &AppState) -> Result<(), String> {
    // Reject before starting a core or changing any persistent state.
    if !platform::capabilities().system_proxy {
        return Err("当前平台仅支持显式 HTTP/SOCKS 代理，尚未支持系统代理接管".into());
    }
    if !state.sysproxy.status().enabled && state.core().status() != CoreStatus::Running {
        start_core(state)?;
        wait_core_running(state)?;
    }
    let _operation = state.core_op.lock().unwrap_or_else(|e| e.into_inner());
    let store = state.store.lock().unwrap();
    let port = store.data().engine.mixed_port;
    let tun_enabled = store.data().engine.tun_enable;
    drop(store);
    if state.sysproxy.status().enabled {
        if tun_enabled {
            state.sysproxy.disable_during_tun()
        } else {
            state.sysproxy.disable()
        }
    } else {
        if state.core().status() != CoreStatus::Running {
            return Err("内核未运行，无法开启系统代理".into());
        }
        if tun_enabled {
            state.sysproxy.enable_during_tun(port)
        } else {
            state.sysproxy.enable(port)
        }
    }
}

#[tauri::command]
async fn sysproxy_toggle(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    notifications::publish(&app, notifications::Notice::new(notifications::Kind::Progress, "tray.proxy", "system-proxy"));
    let result = toggle_sysproxy_state(&state);
    let _ = app.emit("sysproxy://status", state.sysproxy.status());
    refresh_tray(&app);
    notifications::result(&app, "system-proxy", "tray.proxy", result.clone());
    result
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

/// 同步托盘速率显示与内核状态：Running 订阅 /traffic 更新标题；
/// 其他状态退订并清空标题。收养（adopt）路径在托盘创建后也要调一次。
/// 内核状态回调：UI 事件、托盘速率联动、崩溃时系统代理一致性守护。
/// 换核/重装创建新 CoreManager 后必须重新挂载（ensure_engine_binary / core_install）。
fn core_status_callback(handle: AppHandle) -> core_manager::Callback {
    std::sync::Arc::new(move |status| {
        // The configuration transaction owns rollback while core_op is held.
        // A synchronous safe-mode start here would wait for that same lock,
        // while rollback waits for this callback to return.
        if matches!(status, CoreStatus::Crashed(_)) {
            if let Some(state) = handle.try_state::<AppState>() {
                if state
                    .config_transition
                    .load(std::sync::atomic::Ordering::SeqCst)
                {
                    return;
                }
            }
        }
        let _ = handle.emit("core://status", status.clone());
        // 托盘实时速率：Running 订阅更新，停止/崩溃退订清空
        sync_tray_traffic(&handle);
        // 日志落盘：Running 且开关开启时订阅，其余状态退订
        update_logs_persistence(&handle);
        // 托盘节点菜单数据：Running 时后台拉取组与延迟
        if *status == CoreStatus::Running {
            notifications::resolve_core_failure(&handle);
            #[cfg(target_os = "macos")]
            refresh_tray_nodes(&handle, false);
        }
        // 状态一致性守护：内核崩溃时若系统代理还开着，立即还原，
        // 避免系统流量指向已死的代理端口导致用户断网。
        if let CoreStatus::Crashed(msg) = status {
            notifications::result(&handle, "core-failure", "core.crashed", Err(msg.clone()));
            let Some(state) = handle.try_state::<AppState>() else {
                return;
            };
            #[cfg(target_os = "linux")]
            if linux_service::status().is_ok_and(|session| session.is_none_or(|s| !s.active)) {
                let mut store = state.store.lock().unwrap();
                if store.data().engine.tun_enable {
                    store.data_mut().engine.tun_enable = false;
                    if let Err(error) = store.save() {
                        eprintln!("persist disabled Linux TUN: {error}");
                    }
                    let _ = handle.emit("tun://status", false);
                }
            }
            // 状态一致性守护：内核崩溃时还原系统代理，避免断网。
            let proxy_was_on = state.sysproxy.status().enabled;
            if proxy_was_on {
                if let Err(error) = state.sysproxy.disable() {
                    eprintln!("core crash system proxy recovery failed: {error}");
                }
                let _ = handle.emit("sysproxy://status", state.sysproxy.status());
                refresh_tray(&handle);
            }
            // 安全模式：覆写引发的配置解析错误 → 禁用全部覆写并重启一次。
            let config_err = msg.contains("Parse config error");
            let already = state.safe_mode.load(std::sync::atomic::Ordering::SeqCst);
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
                let (port, tun_enabled) = {
                    let store = state.store.lock().unwrap();
                    (store.data().engine.mixed_port, store.data().engine.tun_enable)
                };
                let _ = start_core(&state);
                // 安全模式重启成功后，恢复崩溃前由 Crossbow 控制的代理。
                if proxy_was_on {
                    for _ in 0..100 {
                        if state.core().status() == CoreStatus::Running {
                            let _ = if tun_enabled {
                                state.sysproxy.enable_during_tun(port)
                            } else {
                                state.sysproxy.enable(port)
                            };
                            let _ = handle.emit("sysproxy://status", state.sysproxy.status());
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(200));
                    }
                }
                let mut notice = notifications::Notice::new(notifications::Kind::Warning, "core.safeMode", "core-safe-mode");
                notice.duration = Some(0);
                notice.actions = vec![notifications::Action::Profiles];
                notifications::publish(&handle, notice);
                refresh_tray(&handle);
            }
        }
    })
}

fn sync_tray_traffic(handle: &AppHandle) {
    let Some(state) = handle.try_state::<AppState>() else {
        return;
    };
    if state.core().status() == CoreStatus::Running {
        if let Some((port, secret)) = state.core().controller() {
            let h = handle.clone();
            let _ = state
                .ws
                .subscribe_traffic_tray(port, &secret, move |up, down| {
                    // 速率历史环形缓冲（#181）：托盘通道常驻每秒必推，与页面
                    // 存亡无关，切页期间也持续采样
                    if let Some(state) = h.try_state::<AppState>() {
                        state.traffic_history_push(up, down);
                    }
                    // macOS：富文本双行速率（借道底层 NSStatusItem）；其余平台无操作
                    #[cfg(target_os = "macos")]
                    tray_speed::set_speed(&h, up, down);
                });
            #[cfg(target_os = "macos")]
            ensure_tray_speed_watchdog(handle);
            resubscribe_page_channels(handle);
            return;
        }
    }
    state.ws.unsubscribe_tray_traffic();
    state.traffic_history_clear();
    #[cfg(target_os = "macos")]
    tray_speed::clear(handle);
}

/// 内核（重）启后重订仍被页面需要的通道（traffic/connections/logs）。
/// 页面订阅绑定挂载时的控制器地址——内核一重启端口/密钥全换，旧读线程
/// 会在死端口上无限重连，日志页空、首页图表与连接页停摆都是这个缺口。
/// 借 subscribe 的替换语义，这里用新地址顶替旧通道；意图集（页面挂载
/// 即登记、卸载即移除）保证 Starting 期间挂载失败的页面 Running 后补上。
fn resubscribe_page_channels(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    if state.core().status() != CoreStatus::Running {
        return;
    }
    if !state.ws.any_page_channel_wanted() {
        return;
    }
    let Some((port, secret)) = state.core().controller() else {
        return;
    };
    ensure_page_channel_watchdog(app);
    let h = app.clone();
    let emit = move |ev: &'static str, payload: serde_json::Value| {
        let _ = h.emit(ev, payload);
    };
    let _ = state
        .ws
        .subscribe_traffic(port, &secret, emit.clone());
    let _ = state.ws.subscribe_connections(port, &secret, emit.clone());
    let _ = state.ws.subscribe_logs(port, &secret, emit);
}

/// 页面通道冻结看门狗：内核 Running 但页面 traffic/connections 通道静默
/// 超阈值——典型成因是重启竞态绕过了 Running 补订，旧读线程在换掉的
/// port/secret（死端口）上无限重连——此时用当前控制器地址强制重订。
/// 与托盘通道看门狗同构（ensure_tray_speed_watchdog）；判定逻辑见
/// WsHub::any_page_channel_stale（logs 事件驱动不参与）。#111
fn ensure_page_channel_watchdog(handle: &AppHandle) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static SPAWNED: AtomicBool = AtomicBool::new(false);
    if SPAWNED.swap(true, Ordering::SeqCst) {
        return;
    }
    const STALE_SECS: u64 = 15;
    let h = handle.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            let Some(state) = h.try_state::<AppState>() else {
                continue;
            };
            if state.core().status() != CoreStatus::Running {
                continue;
            }
            if state
                .ws
                .any_page_channel_stale(std::time::Duration::from_secs(STALE_SECS))
            {
                eprintln!(
                    "page watchdog: traffic/connections silent >={STALE_SECS}s, resubscribing"
                );
                resubscribe_page_channels(&h);
            }
        }
    });
}

/// 日志落盘联动（#29）：内核 Running 且开关开启时订阅落盘通道，
/// 其余状态退订。设置开关变化与状态回调都会调用；subscribe 的替换
/// 语义保证设置变化即时生效（新 sink 持有新的 DailyWriter）。
fn update_logs_persistence(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let enabled = {
        let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store.data().logs.save_enabled
    };
    if !(enabled && state.core().status() == CoreStatus::Running) {
        state.ws.unsubscribe("logs-file");
        return;
    }
    let Some((port, secret)) = state.core().controller() else {
        return;
    };
    let mut writer = log_files::DailyWriter::new(log_files::logs_dir(&state.data_dir));
    let _ = state.ws.subscribe_logs_file(port, &secret, move |level, payload| {
        writer.write(level, payload);
    });
}

/// 保留期清理：启动时与每 6 小时各跑一次（按当前设置的天数）。
/// 一次性迁移（#28 标识符切换 com.crossbow.app → com.richard.crossbow）：
/// 新数据目录不存在时整体改名（原子、保留 setuid 与全部内容）；
/// 已存在（Tauri 先建了空目录）则把旧目录里缺的条目逐个移入。
fn migrate_legacy_data_dir(new_dir: &std::path::Path) {
    let Some(parent) = new_dir.parent() else { return };
    let legacy = parent.join("com.crossbow.app");
    if !legacy.is_dir() || new_dir == legacy {
        return;
    }
    if !new_dir.exists() {
        if std::fs::rename(&legacy, new_dir).is_ok() {
            eprintln!("migrated legacy data dir -> {}", new_dir.display());
            return;
        }
    }
    let Ok(entries) = std::fs::read_dir(&legacy) else { return };
    for entry in entries.flatten() {
        let target = new_dir.join(entry.file_name());
        if !target.exists() {
            let _ = std::fs::rename(entry.path(), &target);
        }
    }
}

fn spawn_logs_sweeper(app: &AppHandle) {
    let h = app.clone();
    std::thread::spawn(move || loop {
        let Some(state) = h.try_state::<AppState>() else {
            std::thread::sleep(std::time::Duration::from_secs(600));
            continue;
        };
        let days = state.store.lock().unwrap_or_else(|e| e.into_inner()).data().logs.retention_days;
        let dir = log_files::logs_dir(&state.data_dir);
        drop(state);
        log_files::sweep(&dir, days);
        std::thread::sleep(std::time::Duration::from_secs(6 * 3600));
    });
}

/// Probe actual traffic through the default route. A node-delay benchmark can
/// fail even while normal websites work, so it must not disable a healthy TUN.
fn probe_tun_network() -> Result<(), String> {
    let agent = ureq::AgentBuilder::new()
        .try_proxy_from_env(false)
        .timeout(std::time::Duration::from_secs(5))
        .build();
    let mut last_error = String::new();
    for url in ["https://www.gstatic.com/generate_204", "https://github.com"] {
        match agent.head(url).call() {
            Ok(_) | Err(ureq::Error::Status(_, _)) => return Ok(()),
            Err(error) => last_error = error.to_string(),
        }
    }
    Err(last_error)
}

/// 健康检查重建立。与手动开关共用 core_op，进锁后再读 TUN 标志。
fn reestablish_tun_if_enabled(state: &AppState) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        crate::tun::reestablish_if_enabled(state)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = state;
        Ok(())
    }
}

/// 连通性失败时关 TUN。持锁后再读标志，避免和用户的开关打架。
fn disable_tun_if_enabled(state: &AppState) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        crate::tun::disable_if_enabled(state)
    }
    #[cfg(windows)]
    {
        crate::tun_windows::disable_if_enabled(state)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = state;
        Ok(())
    }
}

/// TUN 健康检查：每 5 秒检查真实网络连通性；连续 3 次失败时关闭 TUN
/// 并通知界面，避免流量持续被送入失效的隧道。
/// macOS 额外检查捕获路由，漂移时走与开关同一把锁的重建立。
#[cfg(any(target_os = "macos", windows))]
fn spawn_tun_health_monitor(app: &AppHandle) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static SPAWNED: AtomicBool = AtomicBool::new(false);
    if SPAWNED.swap(true, Ordering::SeqCst) {
        return;
    }

    const INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
    const MAX_CONSECUTIVE_FAILURES: u8 = 3;
    let handle = app.clone();
    std::thread::spawn(move || {
        let mut failures = 0u8;
        let mut drift_strikes = 0u8;
        let mut reestablish_count = 0u8;
        let mut reestablish_cooldown_until = std::time::Instant::now();
        let mut reported_giveup = false;
        let mut reported_disable_failure = false;
        loop {
            let cycle_started = std::time::Instant::now();
            if let Some(state) = handle.try_state::<AppState>() {
                let tun_enabled = state.store.lock().unwrap().data().engine.tun_enable;
                let core_status = state.core().status();
                if !tun_enabled || core_status == CoreStatus::Starting {
                    failures = 0;
                    drift_strikes = 0;
                    reestablish_count = 0;
                    reported_giveup = false;
                    reported_disable_failure = false;
                } else {
                    // 接管漂移（#154）：捕获路由丢失后，连通性探测可能假绿
                    // （gstatic 存在国内可达地址），显式检查接管本体。漂移
                    // 持续即关开重建立（2 分钟冷却、连续 3 次后停止自动恢复
                    // 并一次性通知），重建立周期跳过连通性探测。
                    let mut skip_probe = false;
                    let cooling = std::time::Instant::now() < reestablish_cooldown_until;
                    #[cfg(target_os = "macos")]
                    let drifted = !cooling && crate::tun::takeover_drift();
                    #[cfg(not(target_os = "macos"))]
                    let drifted = false;
                    if !cooling {
                        if drifted {
                            drift_strikes = drift_strikes.saturating_add(1);
                        } else {
                            drift_strikes = 0;
                            reestablish_count = 0;
                            reported_giveup = false;
                        }
                    }
                    if drifted && drift_strikes >= 2 {
                        if reestablish_count >= 3 {
                            if !reported_giveup {
                                reported_giveup = true;
                                skip_probe = true;
                                let mut notice = notifications::Notice::new(
                                    notifications::Kind::Warning,
                                    "tun.takeoverGiveUp",
                                    "tun-health",
                                );
                                notice.duration = Some(0);
                                notice.actions = vec![notifications::Action::Diagnostics];
                                notifications::publish(&handle, notice);
                            }
                        } else {
                            eprintln!("tun takeover drift persists, re-establishing TUN");
                            drift_strikes = 0;
                            reestablish_count = reestablish_count.saturating_add(1);
                            reestablish_cooldown_until = std::time::Instant::now()
                                + std::time::Duration::from_secs(120);
                            match reestablish_tun_if_enabled(&state) {
                                Ok(()) => {
                                    let still_on = state
                                        .store
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner())
                                        .data()
                                        .engine
                                        .tun_enable;
                                    // 用户在重建立拿到锁之前已经关掉，不要报「已重新建立」。
                                    if still_on {
                                        let _ = handle.emit("tun://status", true);
                                        let _ = handle.emit(
                                            "sysproxy://status",
                                            state.sysproxy.status(),
                                        );
                                        refresh_tray(&handle);
                                        if reestablish_count == 1 {
                                            let notice = notifications::Notice::new(
                                                notifications::Kind::Info,
                                                "tun.reestablished",
                                                "tun-health",
                                            );
                                            notifications::publish(&handle, notice);
                                        }
                                    }
                                }
                                Err(error) => {
                                    eprintln!(
                                        "tun takeover re-establish failed: {error}"
                                    );
                                    failures = failures.saturating_add(1);
                                }
                            }
                            skip_probe = true;
                        }
                    }

                    let probe = if core_status == CoreStatus::Running {
                        // A second proxy client may re-enable a stale macOS
                        // proxy after Crossbow has established its route.
                        if !state.sysproxy.status().enabled
                            && !state.sysproxy.external_endpoints().is_empty()
                        {
                            if let Err(error) = state.sysproxy.suspend_for_tun() {
                                eprintln!("tun system proxy cleanup failed: {error}");
                            }
                        }
                        if skip_probe {
                            // 重建立/放弃周期跳过连通性探测
                            Ok(())
                        } else {
                            probe_tun_network()
                        }
                    } else {
                        // Give the core's automatic restart time to recover.
                        // A persistent Crashed/Stopped state is a dead TUN.
                        Err(format!("TUN 内核未运行：{core_status:?}"))
                    };

                    match probe {
                        Ok(_) => {
                            if !skip_probe && !drifted {
                                notifications::resolve(&handle, "tun-health");
                            }
                            failures = 0;
                            reported_disable_failure = false;
                        }
                        Err(error) => {
                            failures = failures.saturating_add(1);
                            eprintln!(
                                "tun health check failed ({failures}/{MAX_CONSECUTIVE_FAILURES}): {error}"
                            );
                            if failures >= MAX_CONSECUTIVE_FAILURES {
                                let still_enabled =
                                    state.store.lock().unwrap().data().engine.tun_enable;
                                if still_enabled {
                                    match disable_tun_if_enabled(&state) {
                                        Ok(()) => {
                                            failures = 0;
                                            reported_disable_failure = false;
                                            let _ = handle.emit("tun://status", false);
                                            let _ = handle
                                                .emit("sysproxy://status", state.sysproxy.status());
                                            refresh_tray(&handle);
                                            let mut notice = notifications::Notice::new(notifications::Kind::Warning, "tun.disabled", "tun-health");
                                            notice.duration = Some(0);
                                            notice.actions = vec![notifications::Action::Diagnostics];
                                            notifications::publish(&handle, notice);
                                        }
                                        Err(disable_error) => {
                                            failures = 0;
                                            eprintln!(
                                                "tun health recovery could not disable TUN: {disable_error}"
                                            );
                                            let actual = state
                                                .store
                                                .lock()
                                                .unwrap()
                                                .data()
                                                .engine
                                                .tun_enable;
                                            let _ = handle.emit("tun://status", actual);
                                            let _ = handle
                                                .emit("sysproxy://status", state.sysproxy.status());
                                            refresh_tray(&handle);
                                            if !reported_disable_failure {
                                                notifications::result(&handle, "tun-health", "tun.disableFailed", Err(disable_error.clone()));
                                                reported_disable_failure = true;
                                            }
                                        }
                                    }
                                } else {
                                    failures = 0;
                                    reported_disable_failure = false;
                                }
                            }
                        }
                    }
                }
            }
            std::thread::sleep(INTERVAL.saturating_sub(cycle_started.elapsed()));
        }
    });
}

/// 托盘速率看门狗（进程级单例）：内核 Running 期间 /traffic 静默超过阈值
/// 即强制重订。mihomo/sing-box 的 /traffic 即使零流量也每秒推帧，因此
/// 「订阅还在但静默」必然意味着读线程死亡、socket 半开或内核假活——
/// subscribe 的替换语义让重订直接顶替坏通道，冻结在数十秒内自愈。
#[cfg(target_os = "macos")]
fn ensure_tray_speed_watchdog(handle: &AppHandle) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static SPAWNED: AtomicBool = AtomicBool::new(false);
    if SPAWNED.swap(true, Ordering::SeqCst) {
        return;
    }
    const STALE_SECS: u64 = 12;
    let h = handle.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            let Some(state) = h.try_state::<AppState>() else {
                continue;
            };
            if state.core().status() != CoreStatus::Running {
                continue;
            }
            match state.ws.last_message_ago("traffic-tray") {
                Some(silent) if silent.as_secs() >= STALE_SECS => {
                    eprintln!(
                        "tray watchdog: traffic-tray silent {}s, resubscribing",
                        silent.as_secs()
                    );
                    sync_tray_traffic(&h);
                }
                _ => {}
            }
        }
    });
}

/// 后台拉取托盘节点菜单数据：探测主选择组 + 各节点延迟，完成后刷新托盘。
#[cfg(target_os = "macos")]
fn refresh_tray_nodes(handle: &AppHandle, keep_menu_open: bool) {
    let Some(state) = handle.try_state::<AppState>() else {
        return;
    };
    if state.core().status() != CoreStatus::Running {
        return;
    }
    let generation = handle.state::<AppState>().node_menu.lock().unwrap_or_else(|e| e.into_inner()).generation();
    let h = handle.clone();
    std::thread::spawn(move || {
        let Some(state) = h.try_state::<AppState>() else {
            return;
        };
        let Ok(c) = controller_client(&state) else {
            return;
        };
        let groups = match c.groups() {
            Ok(groups) => groups,
            Err(_) => return,
        };
        let mut tray = state.node_menu.lock().unwrap_or_else(|e| e.into_inner());
        if tray.generation() != generation { return; }
        // 主组：已有缓存沿用；否则按订阅声明顺序找第一个有成员的组
        if !tray.has_group() {
            let order = profile_group_order(&h);
            tray.init_group(&|| {
                order
                    .iter()
                    .find(|n| groups.iter().any(|g| g.name == **n && !g.nodes.is_empty()))
                    .cloned()
            });
        }
        let group = tray.group().to_string();
        drop(tray);
        if group.is_empty() {
            return;
        }
        let Some(root) = groups.iter().find(|candidate| candidate.name == group) else {
            return;
        };
        let automatic = root
            .nodes
            .iter()
            .find(|node| node.kind.eq_ignore_ascii_case("URLTest"))
            .and_then(|node| {
                groups
                    .iter()
                    .find(|candidate| candidate.name == node.name)
                    .map(|automatic| {
                        (
                            automatic.name.clone(),
                            automatic.now.clone(),
                            automatic
                                .nodes
                                .iter()
                                .map(|member| (member.name.clone(), member.delay_ms))
                                .collect(),
                        )
                    })
            });
        let mut tray = state.node_menu.lock().unwrap_or_else(|e| e.into_inner());
        if tray.generation() != generation { return; }
        tray.apply_group_data(
            root.now.clone(),
            root.nodes
                .iter()
                .map(|node| (node.name.clone(), node.delay_ms))
                .collect(),
            automatic,
        );
        drop(tray);
        let data = tray_menu_data(&state);
        let menu_updated = keep_menu_open
            && tray_speed::update_node_menu(&h, &data.title, &data.rows, data.testing);
        if !menu_updated {
            refresh_tray(&h);
        }
        // 部分节点还没有延迟数据时自动补一轮测速（静默，不抢菜单交互）
        let missing = {
            let st = h.state::<AppState>();
            let tray = st.node_menu.lock().unwrap_or_else(|e| e.into_inner());
            // 「缺测」= 无延迟且非失败（timeout/failed 徽标的节点已测过，
            // 不能再计入缺失，否则自动补测会无限循环）
            tray.snapshot("", "当前")
                .rows
                .iter()
                .filter(|r| {
                    r.ms.is_none() && r.badge != "timeout" && r.badge != "failed"
                })
                .count()
        };
        #[cfg(target_os = "macos")]
        if missing > 0 {
            run_tray_delay_test(&h);
        }
    });
}

/// 后台执行延迟测速：完成后更新缓存并刷新托盘菜单显示结果。
#[cfg(target_os = "macos")]
fn run_tray_delay_test(handle: &AppHandle) {
    let Some(state) = handle.try_state::<AppState>() else {
        return;
    };
    let generation = {
        let mut tray = state.node_menu.lock().unwrap_or_else(|e| e.into_inner());
        if !tray.begin_test() {
            return; // 已在测速
        }
        tray.generation()
    };
    update_tray_node_menu(handle, true);

    // 并发池测速：每个节点独立请求，完成一个就立即原地刷新菜单——
    // 不再等全组返回（组延迟接口是一次请求测全部，节点多时末尾节点
    // 要等很久才能看到任何结果）。
    let h = handle.clone();
    std::thread::spawn(move || {
        let Some(state) = h.try_state::<AppState>() else {
            return;
        };
        let mut targets: Vec<String> = {
            let tray = state.node_menu.lock().unwrap_or_else(|e| e.into_inner());
            tray.node_names()
        };
        let client = controller_client(&state);
        // 自动选择组（URLTest）的成员并入待测集合（去重）
        if let Ok(client) = &client {
            if let Ok(groups) = client.groups() {
                for g in groups.iter().filter(|g| g.kind.eq_ignore_ascii_case("URLTest")) {
                    for node in &g.nodes {
                        if !targets.contains(&node.name) {
                            targets.push(node.name.clone());
                        }
                    }
                }
            }
        }
        if client.is_err() {
            let mut tray = state.node_menu.lock().unwrap_or_else(|e| e.into_inner());
            if tray.generation() == generation { tray.end_test(); }
            drop(tray);
            let data = tray_menu_data(&state);
            tray_speed::update_node_menu(&h, &data.title, &data.rows, data.testing);
            return;
        }
        let total = targets.len();
        if total == 0 {
            let mut tray = state.node_menu.lock().unwrap_or_else(|e| e.into_inner());
            if tray.generation() == generation {
                tray.end_test();
            }
            drop(tray);
            let data = tray_menu_data(&state);
            tray_speed::update_node_menu(&h, &data.title, &data.rows, data.testing);
            return;
        }

        {
            let mut tray = state.node_menu.lock().unwrap_or_else(|e| e.into_inner());
            if !tray.set_test_targets(generation, &targets) {
                return;
            }
        }

        // 看门狗：预期总时长 = ceil(n/10) * 6s + 余量；超时强制恢复可点态
        let watchdog_handle = h.clone();
        let watchdog_secs = 20 + (total as u64).div_ceil(10) * 6;
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(watchdog_secs));
            let Some(state) = watchdog_handle.try_state::<AppState>() else {
                return;
            };
            let mut tray = state.node_menu.lock().unwrap_or_else(|e| e.into_inner());
            if !tray.expire_test(generation) {
                return; // 已完成或新一轮测速已接管
            }
            drop(tray);
            let data = tray_menu_data(&state);
            tray_speed::update_node_menu(&watchdog_handle, &data.title, &data.rows, data.testing);
        });

        let queue = std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::VecDeque::from(targets),
        ));
        let (tx, rx) = std::sync::mpsc::channel::<(String, Result<u64, String>)>();
        for _ in 0..total.min(10) {
            let queue = queue.clone();
            let tx = tx.clone();
            let h_worker = h.clone();
            std::thread::spawn(move || {
                let client = match h_worker.try_state::<AppState>() {
                    Some(s) => controller_client(&s),
                    None => return,
                };
                loop {
                    let Some(node) = queue
                        .lock()
                        .ok()
                        .and_then(|mut q| q.pop_front())
                    else {
                        break;
                    };
                    let result = match &client {
                        Ok(c) => c.test_node_delay(&node).map_err(|e| {
                            let low = e.to_lowercase();
                            if low.contains("timed out")
                                || low.contains("timeout")
                                || low.contains("deadline")
                                || low.contains("504")
                            {
                                "timeout".to_string()
                            } else {
                                "failed".to_string()
                            }
                        }),
                        Err(_) => Err("failed".to_string()),
                    };
                    if tx.send((node, result)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);

        // 收集器：每条结果立即落库 + 原地刷新（菜单开着时实时可见）
        let mut done = 0usize;
        for (node, result) in rx {
            let Some(state) = h.try_state::<AppState>() else {
                return;
            };
            {
                let mut tray = state.node_menu.lock().unwrap_or_else(|e| e.into_inner());
                if tray.generation() != generation {
                    return; // 新一轮测速已接管，放弃本轮剩余结果
                }
                tray.apply_single_result(&node, result);
            }
            let data = tray_menu_data(&state);
            tray_speed::update_node_menu(&h, &data.title, &data.rows, data.testing);
            done += 1;
            if done >= total {
                break;
            }
        }
        // 全部完成：恢复可点态（与看门狗竞争安全）
        let Some(state) = h.try_state::<AppState>() else {
            return;
        };
        let mut tray = state.node_menu.lock().unwrap_or_else(|e| e.into_inner());
        if tray.generation() != generation {
            return;
        }
        tray.end_test();
        drop(tray);
        let data = tray_menu_data(&state);
        tray_speed::update_node_menu(&h, &data.title, &data.rows, data.testing);
    });
}


#[cfg(target_os = "macos")]
fn update_tray_node_menu(handle: &AppHandle, testing: bool) {
    let Some(state) = handle.try_state::<AppState>() else {
        return;
    };
    let lang = state.store.lock().unwrap().data().ui.lang.clone();
    let data = state.node_menu.lock().unwrap_or_else(|e| e.into_inner()).snapshot(
        &tr(&lang, "节点选择", "Nodes"),
        &tr(&lang, "当前", "Current"),
    );
    tray_speed::update_node_menu(handle, &data.title, &data.rows, testing);
}

#[cfg(target_os = "macos")]
fn tray_menu_data(state: &AppState) -> tray_state::TrayMenuData {
    let lang = state.store.lock().unwrap().data().ui.lang.clone();
    let node_label = tr(&lang, "节点选择", "Nodes");
    let current_label = tr(&lang, "当前", "Current");
    let tray = state.node_menu.lock().unwrap_or_else(|e| e.into_inner());
    tray.snapshot(&node_label, &current_label)
}

fn build_tray_menu(app: &AppHandle) -> Result<Menu<Wry>, tauri::Error> {
    let state = app.state::<AppState>();
    let lang = state.store.lock().unwrap().data().ui.lang.clone();

    let sp = state.sysproxy.status();
    let tun_enabled = state.store.lock().unwrap().data().engine.tun_enable;
    let proxy_label = if !platform::capabilities().system_proxy {
        tr(&lang, "系统代理：当前平台暂不支持", "System proxy: unavailable on this platform")
    } else if sp.enabled {
        format!(
            "{} :{}",
            tr(&lang, "系统代理：已开启", "System Proxy: On"),
            sp.port
        )
    } else {
        tr(&lang, "系统代理：已关闭", "System Proxy: Off")
    };
    let proxy = MenuItem::with_id(app, "toggle-proxy", &proxy_label, platform::capabilities().system_proxy, None::<&str>)?;
    #[cfg(target_os = "macos")]
    let tun_label = if tun_enabled {
        if state.core().status() == CoreStatus::Running {
            tr(&lang, "TUN 模式：运行中", "TUN mode: Running")
        } else {
            tr(&lang, "TUN 已开启 · 内核未运行", "TUN enabled · core stopped")
        }
    } else {
        tr(&lang, "TUN 模式：已关闭", "TUN mode: Off")
    };
    #[cfg(target_os = "macos")]
    let tun_item = MenuItem::with_id(app, "toggle-tun", &tun_label, true, None::<&str>)?;

    let mode = state.mode.lock().unwrap().clone();
    let mk_check = |id: &str, label: &str, checked: bool| {
        CheckMenuItem::with_id(app, id, label, true, checked, None::<&str>)
    };
    let mode_label = format!(
        "{}：{}",
        tr(&lang, "模式", "Mode"),
        tr(
            &lang,
            match mode.as_str() {
                "direct" => "直连",
                "global" => "全局",
                _ => "规则",
            },
            match mode.as_str() {
                "direct" => "Direct",
                "global" => "Global",
                _ => "Rule",
            }
        )
    );
    let mode_menu = SubmenuBuilder::new(app, &mode_label)
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

    // 节点选择子菜单：标题=「主组：当前选中」，首项=延迟测速，
    // 其后为展示节点列表（名称+最近延迟），点击节点即切换。
    // macOS 专属（依赖 NSStatusItem 富文本）；Windows 不展示该子菜单。
    #[cfg(target_os = "macos")]
    let node_menu_data = {
        let data = state.node_menu.lock().unwrap_or_else(|e| e.into_inner()).snapshot(
            &tr(&lang, "节点选择", "Nodes"),
            &tr(&lang, "当前", "Current"),
        );
        Some((data.title, data.rows, data.testing))
    };

    let open = MenuItem::with_id(
        app,
        "open",
        tr(&lang, "打开主窗口", "Open Window"),
        true,
        None::<&str>,
    )?;
    // 无新版本时为 None——菜单里不渲染占位空行
    #[cfg(target_os = "macos")]
    let update_item: Option<tauri::menu::MenuItem<Wry>> =
        update_check::tray_label(app).and_then(|label| {
            MenuItem::with_id(app, "tray-update", &label, true, None::<&str>).ok()
        });
    let quit = MenuItem::with_id(
        app,
        "quit",
        tr(&lang, "退出（还原系统代理）", "Quit (restore proxy)"),
        true,
        None::<&str>,
    )?;

    // 节点子菜单整段 macOS 专属；Windows 直接返回基础菜单
    #[cfg(not(target_os = "macos"))]
    {
        return MenuBuilder::new(app)
            .item(&proxy)
            .separator()
            .item(&mode_menu)
            .item(&profile_menu)
            .separator()
            .item(&open)
            .separator()
            .item(&quit)
            .build();
    }

    #[cfg(target_os = "macos")]
    {
        let (group_menu_label, rows, testing) = node_menu_data.unwrap();
        if rows.is_empty() {
            let mut b = MenuBuilder::new(app)
                .item(&proxy)
                .item(&tun_item)
                .separator()
                .item(&mode_menu)
                .item(&profile_menu)
                .separator();
            if let Some(item) = &update_item {
                b = b.item(item);
            }
            return b.item(&open).separator().item(&quit).build();
        }
        let group_menu_label = tray_speed::format_flag_spacing(&group_menu_label);
        let mut node_builder = SubmenuBuilder::new(app, &group_menu_label);
        let delay_test = MenuItem::with_id(
            app,
            "tray-delay-test",
            &if testing {
                tr(&lang, "延迟测速中…", "Testing latency…")
            } else {
                tr(&lang, "延迟测速", "Test latency")
            },
            !testing,
            None::<&str>,
        )?;
        node_builder = node_builder.item(&delay_test).separator();
        for row in &rows {
            node_builder = node_builder.item(&mk_check(
                &format!("node-{}", row.name),
                &format!("{}\t{}", tray_speed::display_name(&row.label), row.badge),
                row.selected,
            )?);
        }
        let node_menu = node_builder.build()?;

        let mut b = MenuBuilder::new(app)
            .item(&proxy)
            .item(&tun_item)
            .separator()
            .item(&mode_menu)
            .item(&node_menu)
            .item(&profile_menu)
            .separator();
        if let Some(item) = &update_item {
            b = b.item(item);
        }
        b.item(&open)
            .separator()
            .item(&quit)
            .build()
    }
}

fn refresh_tray(app: &AppHandle) {
    // 关键：不能在持有 tray 锁的状态下构建/设置菜单——两者都会同步派发到
    // 主线程执行，而主线程上的托盘标题回调（每秒一次）也要拿这把锁，
    // 持锁派发 = ABBA 死锁（强制退出报告实锤）。先克隆引用再释放锁。
    let tray = app.state::<AppState>().tray.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Some(tray) = tray else { return };
    match build_tray_menu(app) {
        Ok(menu) => {
            let _ = tray.set_menu(Some(menu));
        }
        Err(e) => eprintln!("rebuild tray menu: {e}"),
    }
    // 菜单重建后立即应用等宽列与延迟颜色；测试状态也按当前语言保持一致。
    #[cfg(target_os = "macos")]
    {
        let lang = app
            .state::<AppState>()
            .store
            .lock()
            .unwrap()
            .data()
            .ui
            .lang
            .clone();
        let data = app.state::<AppState>().node_menu.lock().unwrap_or_else(|e| e.into_inner()).snapshot(
            &crate::tr(&lang, "节点选择", "Nodes"),
            &crate::tr(&lang, "当前", "Current"),
        );
        tray_speed::update_node_menu(app, &data.title, &data.rows, data.testing);
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
            let builder = tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title("Crossbow")
            .inner_size(1120.0, 720.0)
            .min_inner_size(880.0, 560.0)
            .center();
            // macOS：Overlay 去掉原生标题栏，内容延伸到窗口顶，避免深色 UI 下
            // 出现灰白边（NSWindow.setBackgroundColor 会毁标题栏外观，勿用）；
            // Windows 走系统默认标题栏
            #[cfg(target_os = "macos")]
            let builder = builder.title_bar_style(tauri::TitleBarStyle::Overlay);
            #[cfg(target_os = "macos")]
            let builder = builder.hidden_title(true);
            let built = builder.build();
            // 重建窗口同样应用深色标题栏
            if let Ok(win) = built.as_ref() {
                let theme = app
                    .try_state::<AppState>()
                    .map(|state| state.store.lock().unwrap().data().ui.theme.clone())
                    .unwrap_or_else(|| "system".to_string());
                apply_window_appearance(win, &theme);
            }
            let _ = built;
            // 内核仍是本进程的子进程（adopted/own），继续归我们管
            if let Some(state) = app.try_state::<AppState>() {
                state
                    .handoff
                    .store(false, std::sync::atomic::Ordering::SeqCst);
            }
            #[cfg(target_os = "macos")]
            let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
        }
    }
}

/// Called after installation succeeds and before requesting an event-loop restart.
/// Persisting the descriptor must succeed before Exit may skip normal cleanup.
pub(crate) fn prepare_update_restart(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let _operation = state.core_op.lock().unwrap_or_else(|e| e.into_inner());
    let handed_off = state.core().prepare_handoff()?.is_some();
    state
        .update_handoff
        .store(handed_off, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

/// 系统代理总开关（托盘与首页共用）。
fn toggle_sysproxy(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    state.core().log_decision(&format!(
        "toggle_sysproxy: enabled={} status={:?}",
        state.sysproxy.status().enabled,
        state.core().status()
    ));
    toggle_sysproxy_state(&state)?;
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
    // A retained notification window means this is no longer the last
    // window. Do not leave the one-shot exit guard armed for the next Cmd-Q.
    state.handoff.store(
        app.get_webview_window("notifications").is_none(),
        std::sync::atomic::Ordering::SeqCst,
    );
    state.ws.unsubscribe_all();
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.destroy(); // 直接销毁，绕过 CloseRequested 防递归
    }
    // Dock 图标隐藏（仅剩顶部状态栏托盘）；cmd+tab 切换能力保留
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
}

fn show_no_more(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.hide();
    }
}

fn show_tray_toggle_error(app: &AppHandle, source: &str, code: &str, error: String) {
    notifications::result(app, source, code, Err(error));
}

fn handle_menu(app: &AppHandle, event: tauri::menu::MenuEvent) {
    let id = event.id().as_ref();
    let result = match id {
        "toggle-proxy" => {
            let handle = app.clone();
            std::thread::spawn(move || {
                notifications::publish(&handle, notifications::Notice::new(notifications::Kind::Progress, "tray.proxy", "tray-proxy"));
                if let Err(error) = toggle_sysproxy(&handle) {
                    eprintln!("tray system proxy toggle failed: {error}");
                    show_tray_toggle_error(&handle, "tray-proxy", "tray.proxy", error);
                } else { notifications::result(&handle, "tray-proxy", "tray.proxy", Ok(())); }
            });
            Ok(())
        }
        "toggle-tun" => {
            #[cfg(target_os = "windows")]
            {
                let handle = app.clone();
                std::thread::spawn(move || {
                    notifications::publish(&handle, notifications::Notice::new(notifications::Kind::Progress, "tray.tun", "tray-tun"));
                    let state = handle.state::<AppState>();
                    let enabled = state.store.lock().unwrap().data().engine.tun_enable;
                    let result = crate::tun_windows::toggle_tun(&state, !enabled);
                    let actual = state.store.lock().unwrap().data().engine.tun_enable;
                    let _ = handle.emit("tun://status", actual);
                    let _ = handle.emit("sysproxy://status", state.sysproxy.status());
                    refresh_tray(&handle);
                    if let Err(error) = result {
                        eprintln!("tray TUN toggle failed: {error}");
                        show_tray_toggle_error(&handle, "tray-tun", "tray.tun", error);
                    } else { notifications::result(&handle, "tray-tun", "tray.tun", Ok(())); }
                });
            }
            #[cfg(target_os = "macos")]
            {
                let handle = app.clone();
                std::thread::spawn(move || {
                    notifications::publish(&handle, notifications::Notice::new(notifications::Kind::Progress, "tray.tun", "tray-tun"));
                    let state = handle.state::<AppState>();
                    let enabled = state.store.lock().unwrap().data().engine.tun_enable;
                    let result = crate::tun::toggle_tun(&state, !enabled);
                    let actual = state.store.lock().unwrap().data().engine.tun_enable;
                    let _ = handle.emit("tun://status", actual);
                    let _ = handle.emit("sysproxy://status", state.sysproxy.status());
                    refresh_tray(&handle);
                    if let Err(error) = result {
                        eprintln!("tray TUN toggle failed: {error}");
                        show_tray_toggle_error(&handle, "tray-tun", "tray.tun", error);
                    } else { notifications::result(&handle, "tray-tun", "tray.tun", Ok(())); }
                });
            }
            Ok(())
        }
        "mode-direct" | "mode-rule" | "mode-global" => {
            let mode = id.strip_prefix("mode-").unwrap().to_string();
            let handle = app.clone();
            std::thread::spawn(move || {
                notifications::publish(&handle, notifications::Notice::new(notifications::Kind::Progress, "core.mode", "tray-mode"));
                let result = set_core_mode(handle.clone(), handle.state::<AppState>(), mode);
                notifications::result(&handle, "tray-mode", "core.mode", result);
            });
            Ok(())
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
        "tray-update" => {
            show_main_window(app);
            let _ = app.emit("ui://goto", serde_json::json!({ "page": "settings" }));
            Ok(())
        }
        "tray-delay-test" => {
            #[cfg(target_os = "macos")]
            run_tray_delay_test(app);
            Ok(())
        }
        other => {
            if let Some(name) = other.strip_prefix("node-") {
                let group = app
                    .state::<AppState>()
                    .node_menu
                    .lock()
                    .unwrap()
                    .group()
                    .to_string();
                let h = app.clone();
                let name = name.to_string();
                std::thread::spawn(move || {
                    let result = controller_client(&h.state::<AppState>())
                        .and_then(|c| c.select_proxy(&group, &name));
                    if let Err(error) = result {
                        show_tray_toggle_error(&h, "tray-node", "tray.node", error);
                    } else {
                        // 节点子菜单仅 macOS 构建；选择成功后就地刷新打开中的菜单。
                        #[cfg(target_os = "macos")]
                        refresh_tray_nodes(&h, true);
                    }
                });
                Ok(())
            } else if let Some(pid) = other.strip_prefix("profile-") {
                let handle = app.clone();
                let profile = pid.to_string();
                std::thread::spawn(move || {
                    notifications::publish(&handle, notifications::Notice::new(notifications::Kind::Progress, "tray.profile", "tray-profile"));
                    let result = tauri::async_runtime::block_on(set_active_profile(handle.clone(), handle.state::<AppState>(), profile));
                    notifications::result(&handle, "tray-profile", "tray.profile", result);
                });
                Ok(())
            } else {
                Ok(())
            }
        }
    };
    if let Err(e) = result {
        show_tray_toggle_error(app, "tray-action", "tray.action", e);
    }
}

// ---------- 应用入口 ----------

/// 致命启动错误可见化：弹窗 + stderr（CI/服务场景无 GUI 时仍有日志）。
fn show_fatal(msg: &str) {
    eprintln!("crossbow fatal: {msg}");
    let _ = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("Crossbow 启动失败")
        .set_description(msg)
        .show();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// Windows：更新安装器以提权 token 重启应用；若 UAC 中选择了另一个
/// 管理员账号，提权实例会读那个账号的 AppData，订阅等数据看似全丢（#252）。
/// GUI 不需要管理员（TUN 由 LocalSystem 服务承载），发现被提权就用
/// explorer.exe 派生交互用户会话的降权实例并退出。
#[cfg(target_os = "windows")]
fn deelevate_if_needed() {
    const SKIP_ENV: &str = "CROSSBOW_DEELEVATED";
    if std::env::var_os(SKIP_ENV).is_some() || !is_elevated() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let spawned = std::process::Command::new("explorer.exe")
        .arg(&exe)
        .env(SKIP_ENV, "1")
        .spawn()
        .is_ok();
    if spawned {
        eprintln!(
            "elevated process detected; relaunched de-elevated via explorer to keep the interactive user's data dir"
        );
        // 不做清理直接退出：pidfile 里是已死 pid，降权实例的保险检查能放行；
        // 本实例尚未打开 store/内核，无孤儿资源。
        std::process::exit(0);
    }
    // explorer 派生失败（shell 未运行等）→ 继续提权运行，可用性优先。
    eprintln!("deelevate relaunch failed; continuing elevated");
}

#[cfg(target_os = "windows")]
fn is_elevated() -> bool {
    // windows-sys 0.59 未导出 OpenProcessToken；IsUserAnAdmin（shell32）判定
    // 当前令牌是否处于管理员组——UAC 拆分令牌（未提权）会正确返回 false。
    unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0 }
}

pub fn run() {
    #[cfg(target_os = "windows")]
    deelevate_if_needed();
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
            notifications::install(app.handle());
            // 一次性迁移：旧标识符（com.crossbow.app）数据目录 → 新标识符
            let legacy_target = app.path().app_data_dir().map_err(|e| e.to_string())?;
            migrate_legacy_data_dir(&legacy_target);
            let data_dir = app.path().app_data_dir()?;
            // First launch has no legacy directory. Create it before app.pid;
            // Store::open happens later and cannot prepare the pidfile parent.
            std::fs::create_dir_all(&data_dir)?;

            // 双实例保险：single-instance 插件依赖对端实例的事件循环还活着
            // （主线程卡死/退出被误拦的旧实例接不住通知）。pidfile 不依赖对方，
            // 只要 pid 存活就拒绝第二个实例，避免两实例互杀内核。
            let lock_path = data_dir.join("app.pid");
            if let Ok(old) = std::fs::read_to_string(&lock_path) {
                if let Ok(pid) = old.trim().parse::<u32>() {
                    if pid_alive(pid) {
                        return Err(format!(
                            "另一个 Crossbow 实例正在运行 (pid {pid})，退出它后再启动"
                        )
                        .into());
                    }
                }
            }
            std::fs::write(&lock_path, std::process::id().to_string())?;

            let mut store = Store::open(&data_dir)
                .map_err(|e| std::io::Error::other(format!("open store: {e}")))?;
            // Linux network authorization belongs to this desktop session.
            // A previous TUN preference never silently takes over after login.
            #[cfg(target_os = "linux")]
            if store.data().engine.tun_enable {
                store.data_mut().engine.tun_enable = false;
                store.save().map_err(|e| std::io::Error::other(e.to_string()))?;
            }
            let update_settings_init = store.data().update.clone();

            // 主窗口使用与应用界面一致的系统外观，避免浅色窗口描边。
            if let Some(win) = app.get_webview_window("main") {
                apply_window_appearance(&win, &store.data().ui.theme);
            }

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
            core.set_callback(core_status_callback(handle.clone()));

            // 状态一致性守护：
            // 1) 尝试收养上一实例轻量交接的内核（描述符 + 控制器探测）
            // 2) 收养成功 → 系统代理状态从 journal 恢复（不还原设置）
            // 3) 收养失败 → 按异常退出处理：全量还原系统代理
            #[cfg(target_os = "macos")]
            let backend = sysproxy::NetworkSetup;
            #[cfg(windows)]
            let backend = sysproxy::WinInet;
            #[cfg(not(any(target_os = "macos", windows)))]
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

            // A handed-over core can still have TUN enabled after a prior
            // upgrade cleared the preference. Reconcile from the live API.
            #[cfg(target_os = "macos")]
            if adopted && core_download::is_core_bin(&core.binary_path(), "mihomo") {
                if let Some((port, secret)) = core.controller() {
                    match (mihomo_api::Controller { port, secret }).tun_enabled() {
                        Ok(Some(enabled)) if store.data().engine.tun_enable != enabled => {
                            store.data_mut().engine.tun_enable = enabled;
                            store.save()?;
                        }
                        Err(_) => eprintln!(
                            "could not reconcile adopted TUN state; keeping saved preference"
                        ),
                        _ => {}
                    }
                }
            }

            // The saved preference alone does not mean a tunnel exists. On a
            // cold start without an adopted core, require an explicit new TUN
            // start so Settings cannot claim that an inactive tunnel is on.
            if store.data().engine.tun_enable && !adopted {
                store.data_mut().engine.tun_enable = false;
                store.save()?;
                eprintln!("cleared stale TUN flag: no running core was adopted");
            }
            if !adopted {
                if let Err(error) = sysproxy.recover_tun_without_core() {
                    eprintln!("could not restore system proxy after stale TUN: {error}");
                }
            }

            app.manage(AppState {
                handoff: std::sync::atomic::AtomicBool::new(false),
                update_handoff: std::sync::atomic::AtomicBool::new(false),
                safe_mode: std::sync::atomic::AtomicBool::new(false),
                config_transition: std::sync::atomic::AtomicBool::new(false),
                store: Mutex::new(store),
                core_slot: Mutex::new(core),
                core_op: Mutex::new(()),
                sysproxy,
                mode: Mutex::new("rule".to_string()),
                tray: Mutex::new(None),
                ws: WsHub::default(),
                data_dir: data_dir.clone(),
                app: app.handle().clone(),
                update: Mutex::new(update_settings_init),
                node_menu: Mutex::new(tray_state::TrayState::default()),
                traffic_history: Mutex::new(std::collections::VecDeque::new()),
            });

            #[cfg(any(target_os = "macos", windows))]
            spawn_tun_health_monitor(app.handle());

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
                // macOS uses a template; Windows needs a visible colored tray icon.
                .icon(if cfg!(target_os = "macos") {
                    tauri::include_image!("icons/tray-icon.png")
                } else {
                    tauri::include_image!("icons/32x32.png")
                })
                .icon_as_template(cfg!(target_os = "macos"))
                .menu(&menu)
                .show_menu_on_left_click(true)
                .tooltip("Crossbow")
                .on_menu_event(handle_menu)
                .build(app)?;
            *app.state::<AppState>().tray.lock().unwrap_or_else(|e| e.into_inner()) = Some(tray);
            #[cfg(target_os = "macos")]
            tray_speed::install_menu_observer(app.handle());

            // 收养的外部内核不走状态回调：这里补挂托盘速率与节点菜单数据
            sync_tray_traffic(app.handle());
            // 收养不走状态回调：日志落盘订阅同样需要补挂
            update_logs_persistence(app.handle());
            spawn_logs_sweeper(app.handle());
            // 更新检查线程（启动 30s 后首查，之后每 6h）
            update_check::spawn_checker(app.handle());
            #[cfg(target_os = "macos")]
            refresh_tray_nodes(app.handle(), false);

            Ok(())
        })
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                notifications::window_destroyed(window.app_handle(), window.label());
            }
            if window.label() != "main" { return; }
            #[cfg(windows)]
            if let tauri::WindowEvent::ThemeChanged(theme) = event {
                let follows_system = window.app_handle().try_state::<AppState>()
                    .is_some_and(|state| state.store.lock().unwrap_or_else(|e| e.into_inner()).data().ui.theme == "system");
                if follows_system {
                    let dark = *theme == tauri::Theme::Dark;
                    if let Some(main) = window.app_handle().get_webview_window("main") {
                        windows_appearance::apply(&main, dark);
                        let (r,g,b) = if dark { (10,12,16) } else { (246,247,249) };
                        let _ = main.set_background_color(Some(tauri::utils::config::Color(r,g,b,255)));
                    }
                }
            }
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
            platform_capabilities,
            notifications::notification_publish,
            notifications::notification_snapshot,
            notifications::notification_dismiss,
            notifications::notification_pause,
            notifications::notification_mark_read,
            notifications::notification_resolve,
            notifications::notification_clear_completed,
            notifications::notification_restore_completed,
            notifications::notification_action,
            notifications::notification_ack_navigation,
            notifications::notification_resize,
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
            traffic_history,
            subscribe_connections,
            unsubscribe_connections,
            subscribe_logs,
            unsubscribe_logs,
            get_ui_settings,
            get_engine_config,
            set_theme,
            logs_settings,
            set_logs_save,
            set_logs_retention,
            open_logs_dir,
            update_state_get,
            coexist_detect,
            update_check_now,
            update_start_download,
            update_install,
            set_update_auto_check,
            update_settings_get,
            set_update_auto_download,
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
            set_tun,
            tun_status,
            diag_log,
            run_diagnosis,
            test_site,
            current_outbound,
            close_connection,
            proxies_snapshot,
            select_proxy,
            test_group_delay,
            test_node_delay,
        ])
        .build(tauri::generate_context!())
        .unwrap_or_else(|e| {
            // 启动失败必须可见（setup 报错最终走到这里）：Windows 上静默退出＝
            // 闪现即退，无从排查。弹窗给出原因。
            show_fatal(&e.to_string());
            panic!("error while building tauri application: {e}");
        })
        .run(|app, event| {
            match event {
                tauri::RunEvent::Ready => {
                    #[cfg(target_os = "macos")]
                    if let Err(error) = macos_icon::install() {
                        // Cosmetic fallback: keep the system icon, never fail startup.
                        eprintln!("[crossbow] {error}");
                    }
                }
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
                        // 退出时不允许 root 内核补杀弹授权框：杀不掉就留给系统，
                        // 下次启动的收养/预绑定检查会处理。
                        core_manager::ROOT_KILL_PROMPT_DISABLED
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                        state.ws.unsubscribe_all();
                        let _ = std::fs::remove_file(state.data_dir.join("app.pid"));
                        let handoff = state.handoff.load(std::sync::atomic::Ordering::SeqCst);
                        let updating = state
                            .update_handoff
                            .load(std::sync::atomic::Ordering::SeqCst);
                        if !handoff && !updating {
                            let tun_enabled = state.store.lock().unwrap().data().engine.tun_enable;
                            if tun_enabled {
                                // 特权内核停掉之后，立刻按记住的记录还原系统代理。
                                if state.core().stop() {
                                    recover_network_if_core_dead(&state);
                                }
                            } else {
                                state.core().stop();
                                recover_network_if_core_dead(&state);
                            }
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
    fn failed_save_restores_memory_and_preserves_previous_disk_state() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::open(directory.path()).unwrap();
        store.data_mut().engine.mixed_port = 7897;
        store.save().unwrap();
        let before = store.data().clone();
        store.data_mut().engine.mixed_port = 7898;
        // Inject a deterministic I/O failure, also when tests run as root.
        std::fs::create_dir(directory.path().join("store.json.tmp")).unwrap();
        let store = Mutex::new(store);
        assert!(restore_store(&store, &before).is_err());
        assert_eq!(store.lock().unwrap().data().engine.mixed_port, 7897);
        assert_eq!(Store::open(directory.path()).unwrap().data().engine.mixed_port, 7897);
    }

    #[test]
    fn profile_name_from_path_strips_extension() {
        assert_eq!(profile_name_from_path("/a/b/my-config.yaml"), "my-config");
        assert_eq!(profile_name_from_path("配置.yml"), "配置");
        assert_eq!(profile_name_from_path("noext"), "noext");
        assert_eq!(profile_name_from_path("/a/b/.hidden"), ".hidden");
    }
}
