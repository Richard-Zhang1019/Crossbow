//! Crossbow 桌面壳层（macOS + Windows，Tauri 2）。
//!
//! M0-S4 职责：在 S3（系统代理/托盘/关窗驻留）之上加入 WS 数据桥
//! （traffic/connections/logs 节流聚合）、设置（端口/局域网/主题/自启/
//! 内核版本）、深链接导入与单实例。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::engine::bridge::WsHub;
use crossbow_core::subscription::{fetch_subscription, now_secs, validate_profile_content};
use crossbow_core::{Profile, ProfileKind, RuntimeConfig, Store, UiSettings};
#[cfg(not(target_os = "macos"))]
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as _};
use tauri_plugin_deep_link::DeepLinkExt as _;

// 子模块（commands/* 等）经 `use crate::*` 依赖这些名字——pub(crate) 重导出
// 保持旧路径可用（#423 拆分）。
pub(crate) use crate::engine::download::{
    resolve_core_binary, resolve_core_for, CoreBinaryInfo, InstallProgress,
};
pub(crate) use crate::engine::manager::{CoreManager, CoreStatus};
pub(crate) use crate::network::sysproxy::{SysProxyManager, SysProxyStatus};
pub(crate) use commands::helpers::*;
pub(crate) use engine::flow::*;
pub(crate) use lifecycle::exit_flow::*;
pub(crate) use tray::wiring::*;

mod app_state;
mod commands;
pub(crate) use app_state::*;
use commands::activity::*;
use commands::core::*;
use commands::monitor::*;
use commands::overrides::*;
use commands::profiles::*;
use commands::proxies::*;
use commands::settings::*;
use commands::takeover::*;
use commands::update::*;
pub(crate) mod activity_reporting;
pub(crate) mod diagnostics;
pub(crate) mod engine;
pub(crate) mod lifecycle;
pub(crate) mod network;
pub(crate) mod notifications;
pub(crate) mod platform;
pub(crate) mod tray;

// 跨域单件：进程管理、调度、服务端 IPC、应用更新、维护命令。
pub(crate) mod process;
pub(crate) mod scheduler;
#[cfg(windows)]
pub(crate) mod service_client;
pub(crate) mod update_check;
pub(crate) mod workflow;
/// Match the AppKit window border to the application's theme.
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
    crate::platform::windows_appearance::apply(win, dark);
    let (r, g, b) = if dark { (10, 12, 16) } else { (246, 247, 249) };
    let _ = win.set_background_color(Some(tauri::utils::config::Color(r, g, b, 255)));
}

/// 活动订阅里 proxy-groups 的声明顺序（代理页排序与托盘主组探测共用）。
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn profile_group_order(handle: &AppHandle) -> Vec<String> {
    let Some(state) = handle.try_state::<AppState>() else {
        return Vec::new();
    };
    let content = {
        let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store
            .data()
            .active_profile
            .clone()
            .and_then(|id| store.data().profile(&id).map(|p| p.content.clone()))
    };
    let Some(content) = content else {
        return Vec::new();
    };
    serde_yaml::from_str::<serde_yaml::Value>(&content)
        .ok()
        .and_then(|v| {
            v.get("proxy-groups")
                .and_then(|g| g.as_sequence())
                .map(|seq| {
                    seq.iter()
                        .filter_map(|g| g.get("name").and_then(|n| n.as_str()).map(String::from))
                        .collect()
                })
        })
        .unwrap_or_default()
}

/// Keep the lock file in place. Unlinking it could allow another process to
/// lock a new inode while the first process still holds the original lock.
fn lock_app_instance(data_dir: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::fs::{OpenOptions, TryLockError};
    use std::io::{Error, ErrorKind};

    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(data_dir.join("app.lock"))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            return Err(Error::new(
                ErrorKind::WouldBlock,
                "另一个 Crossbow 实例正在运行或启动",
            ));
        }
        Err(TryLockError::Error(error)) => return Err(error),
    }

    // Older versions do not take app.lock. Keep honoring their live PID.
    match std::fs::read(data_dir.join("app.pid")) {
        Ok(content) => {
            let pid = std::str::from_utf8(&content)
                .ok()
                .and_then(|value| value.trim().parse::<u32>().ok())
                .filter(|&pid| pid > 0 && (!cfg!(unix) || pid <= i32::MAX as u32))
                .filter(|&pid| crate::engine::manager::pid_is_alive(pid));
            if let Some(pid) = pid {
                return Err(Error::new(
                    ErrorKind::WouldBlock,
                    format!("另一个 Crossbow 实例正在运行 (pid {pid})，退出它后再启动"),
                ));
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    Ok(lock)
}

fn acquire_app_instance(data_dir: &std::path::Path) -> std::io::Result<std::fs::File> {
    let lock = lock_app_instance(data_dir)?;
    crate::platform::fs_perms::write_private(
        &data_dir.join("app.pid"),
        std::process::id().to_string().as_bytes(),
    )?;
    Ok(lock)
}

const DEEP_LINK_CONFIRM_TIMEOUT: Duration = Duration::from_secs(60);
static DEEP_LINK_IMPORT_ACTIVE: AtomicBool = AtomicBool::new(false);

struct DeepLinkImportGuard<'a>(&'a AtomicBool);

impl<'a> DeepLinkImportGuard<'a> {
    fn acquire(active: &'a AtomicBool) -> Option<Self> {
        active
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()?;
        Some(Self(active))
    }
}

impl Drop for DeepLinkImportGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn deep_link_import_target(raw: &str) -> Option<url::Url> {
    let link = url::Url::parse(raw).ok()?;
    if link.scheme() != "crossbow" || link.host_str() != Some("import") {
        return None;
    }
    let (_, target) = link.query_pairs().find(|(key, _)| key == "url")?;
    let target = url::Url::parse(&target).ok()?;
    (matches!(target.scheme(), "http" | "https") && target.has_host()).then_some(target)
}

fn deep_link_confirmation_accepted(confirmed: bool, elapsed: Duration) -> bool {
    confirmed && elapsed < DEEP_LINK_CONFIRM_TIMEOUT
}

fn confirm_deep_link_import(state: &AppState, origin: &str) -> bool {
    let Ok(store) = state.store.lock() else {
        return false;
    };
    let confirm_enabled = store.data().ui.deep_link_import_confirm;
    let zh = store.data().ui.lang == "zh";
    drop(store);
    if !confirm_enabled {
        return true;
    }

    let (title, hint) = if zh {
        (
            "Crossbow 导入确认",
            "是否从上方来源导入订阅？此操作可能改变流量走向。请在 60 秒内确认。\n可在「设置 → 应用与更新」关闭此确认。",
        )
    } else {
        (
            "Crossbow import confirmation",
            "Import a subscription from the source above? This may change how your traffic is routed. Confirm within 60 seconds.\nYou can turn this off in Settings → Application & updates.",
        )
    };
    let description = format!("{origin}\n\n{hint}");
    let opened_at = Instant::now();
    let confirmed = show_deep_link_confirmation(title, &description, zh);
    deep_link_confirmation_accepted(confirmed, opened_at.elapsed())
}

#[cfg(not(windows))]
fn show_deep_link_confirmation(title: &str, description: &str, zh: bool) -> bool {
    let (cancel, import) = if zh {
        ("取消", "导入")
    } else {
        ("Cancel", "Import")
    };
    // Put the safe action first so the dialog's default cannot import.
    let result = rfd::MessageDialog::new()
        .set_title(title)
        .set_description(description)
        .set_level(rfd::MessageLevel::Warning)
        .set_buttons(rfd::MessageButtons::OkCancelCustom(
            cancel.into(),
            import.into(),
        ))
        .show();
    matches!(result, rfd::MessageDialogResult::Custom(label) if label == import)
}

#[cfg(windows)]
fn show_deep_link_confirmation(title: &str, description: &str, _zh: bool) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDOK, MB_DEFBUTTON2, MB_ICONWARNING, MB_OKCANCEL, MB_SETFOREGROUND,
    };

    let title: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let description: Vec<u16> = description
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // The UTF-16 buffers are NUL-terminated and live through the synchronous call.
    let result = unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            description.as_ptr(),
            title.as_ptr(),
            MB_OKCANCEL | MB_DEFBUTTON2 | MB_ICONWARNING | MB_SETFOREGROUND,
        )
    };
    result == IDOK
}

fn handle_crossbow_url(app: &AppHandle, raw: &str) {
    let Some(target) = deep_link_import_target(raw) else {
        return;
    };
    let Some(guard) = DeepLinkImportGuard::acquire(&DEEP_LINK_IMPORT_ACTIVE) else {
        return;
    };
    let app = app.clone();
    let _ = std::thread::Builder::new()
        .name("deep-link-import".into())
        .spawn(move || {
            let _guard = guard;
            let Some(state) = app.try_state::<AppState>() else {
                return;
            };
            let origin = target.origin().ascii_serialization();
            if !confirm_deep_link_import(&state, &origin) {
                return;
            }
            show_main_window(&app);
            match import_url_blocking(&state, target.as_str(), None) {
                Ok(profile) => {
                    let mut notice = notifications::Notice::new(
                        notifications::Kind::Success,
                        "import.done",
                        "deep-link-import",
                    );
                    notice.description = origin;
                    let is_active = state
                        .store
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .data()
                        .active_profile
                        .as_deref()
                        == Some(profile.id.as_str());
                    if is_active {
                        notice.actions = vec![notifications::Action::ContinueSetup];
                        let _ = app.emit("profile://switched", profile.id.as_str());
                    }
                    notifications::publish(&app, notice);
                    refresh_tray(&app);
                }
                Err(e) => {
                    notifications::result(
                        &app,
                        "deep-link-import",
                        "import.failed",
                        Err(format!("{origin}: {e}")),
                    );
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

/// 总开关：开启 = 确保内核运行 + 设置系统代理；关闭 = 还原系统代理（内核保持）。
fn toggle_sysproxy_state(state: &AppState) -> Result<(), String> {
    state.exit.ensure_open()?;
    // Reject before starting a core or changing any persistent state.
    if !platform::capabilities().system_proxy {
        return Err("当前平台仅支持显式 HTTP/SOCKS 代理，尚未支持系统代理接管".into());
    }
    if !state.sysproxy.status().enabled && state.core().status() != CoreStatus::Running {
        start_core(state)?;
        wait_core_running(state)?;
    }
    let _operation = state.core_op.lock().unwrap_or_else(|e| e.into_inner());
    state.exit.ensure_open()?;
    let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
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
            state.sysproxy.enable_during_tun(port)?;
        } else {
            state.sysproxy.enable(port)?;
        }
        // 原生授权期间内核可能已退出；完成后复查，避免代理指向失效端口。
        if state.core().status() != CoreStatus::Running {
            let recovery = if tun_enabled {
                state.sysproxy.disable_during_tun()
            } else {
                state.sysproxy.disable()
            };
            return match recovery {
                Ok(()) => Err("管理员授权期间内核已停止，系统代理已恢复".into()),
                Err(error) => Err(format!("管理员授权期间内核已停止；代理恢复失败：{error}")),
            };
        }
        Ok(())
    }
}

fn publish_proxy_toggle_result(app: &AppHandle, source: &str, result: Result<(), String>) {
    if let Err(error) = &result {
        if ["mihomo", "sing-box"]
            .iter()
            .any(|engine| *error == missing_core_error(engine))
        {
            let mut notice =
                notifications::Notice::new(notifications::Kind::Error, "core.missing", source);
            notice.details = error.clone();
            notice.actions = vec![notifications::Action::Settings];
            notifications::publish(app, notice);
            return;
        }
    }
    notifications::result(app, source, "tray.proxy", result);
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
pub fn run() {
    // The tray helper only hosts AppKit. Startup diagnostics and proxy
    // recovery belong to the desktop process that owns the app state.
    #[cfg(target_os = "macos")]
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new(crate::tray::helper::ARG))
    {
        if let Err(error) = crate::tray::helper::run() {
            eprintln!("macOS tray helper: {error}");
            std::process::exit(1);
        }
        return;
    }
    install_panic_hook();
    clear_stale_system_proxy_early();
    #[cfg(target_os = "windows")]
    deelevate_if_needed();
    tauri::Builder::default()
        // 单实例必须最先注册：二次启动的 argv 里可能带 crossbow:// 深链接。
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if let Some(url) = argv
                .iter()
                .skip(1)
                .find(|arg| url::Url::parse(arg).is_ok_and(|url| url.scheme() == "crossbow"))
            {
                handle_crossbow_url(app, url);
            } else {
                show_main_window(app);
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

            // Keep ownership until the exit event (or process termination).
            let instance_lock = acquire_app_instance(&data_dir)?;
            tidy_runtime_files(&data_dir);

            let mut store = Store::open(&data_dir)
                .map_err(|e| std::io::Error::other(format!("open store: {e}")))?;
            // Linux network authorization belongs to this desktop session.
            // A previous TUN preference never silently takes over after login.
            #[cfg(target_os = "linux")]
            if store.data().engine.tun_enable {
                store.data_mut().engine.tun_enable = false;
                store
                    .save()
                    .map_err(|e| std::io::Error::other(e.to_string()))?;
            }
            let update_settings_init = store.data().update.clone();

            // 主窗口使用与应用界面一致的系统外观，避免浅色窗口描边。
            if let Some(win) = app.get_webview_window("main") {
                apply_window_appearance(&win, &store.data().ui.theme);
                // 会话结束（关机/注销）兜底：还原系统代理（#334 review P3）
                #[cfg(target_os = "windows")]
                if let Ok(hwnd) = win.hwnd() {
                    // tauri 的 hwnd() 返回 windows crate 的 HWND 结构体；
                    // windows-sys 侧接口需要裸指针
                    crate::lifecycle::windows_session_end::install(app.handle(), hwnd.0);
                }
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
            let backend = crate::network::sysproxy::NetworkSetup;
            #[cfg(windows)]
            let backend = crate::network::sysproxy::WinInet;
            #[cfg(not(any(target_os = "macos", windows)))]
            let backend = crate::network::sysproxy::UnsupportedBackend;
            let sysproxy = SysProxyManager::new(backend, data_dir.join("sysproxy-journal.json"));
            let adopted = core.adopt_external();
            if !adopted {
                core.trim_startup_log();
            }
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
            if adopted && crate::engine::download::is_core_bin(&core.binary_path(), "mihomo") {
                if let Some((port, secret)) = core.controller() {
                    match (crate::engine::api::Controller { port, secret }).tun_enabled() {
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
                instance_lock: Mutex::new(Some(instance_lock)),
                exit: crate::lifecycle::shutdown::ExitState::default(),
                handoff: std::sync::atomic::AtomicBool::new(false),
                update_handoff: std::sync::atomic::AtomicBool::new(false),
                safe_mode: std::sync::atomic::AtomicBool::new(false),
                config_transition: std::sync::atomic::AtomicBool::new(false),
                store: Mutex::new(store),
                core_slot: Mutex::new(core),
                core_op: Mutex::new(()),
                core_install_op: tauri::async_runtime::Mutex::new(()),
                core_restart_op: tauri::async_runtime::Mutex::new(()),
                proxy_op: Mutex::new(()),
                sysproxy,
                mode: Mutex::new("rule".to_string()),
                #[cfg(not(target_os = "macos"))]
                tray: Mutex::new(None),
                ws: WsHub::default(),
                activity_reporting: activity_reporting::Reporter::open(&data_dir),
                data_dir: data_dir.clone(),
                app: app.handle().clone(),
                update: Mutex::new(update_settings_init),
                node_menu: Mutex::new(crate::tray::state::TrayState::default()),
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
                // Windows and Linux record the startup URL before setup registers
                // this listener. Replay it here. The single-instance callback handles
                // later argv deliveries.
                #[cfg(any(target_os = "linux", windows))]
                if let Some(urls) = app.deep_link().get_current()? {
                    for url in urls {
                        handle_crossbow_url(app.handle(), url.as_str());
                    }
                }
            }

            // 托盘
            #[cfg(target_os = "macos")]
            crate::tray::speed::install(app.handle())?;
            #[cfg(not(target_os = "macos"))]
            {
                let menu =
                    crate::tray::menu::build(app.handle(), &tray_menu_entries(app.handle()))?;
                let tray = TrayIconBuilder::new()
                    .icon(tauri::include_image!("icons/32x32.png"))
                    .menu(&menu)
                    .show_menu_on_left_click(true)
                    .tooltip("Crossbow")
                    .on_menu_event(|app, event| handle_menu(app, event.id().as_ref()))
                    .build(app)?;
                *app.state::<AppState>()
                    .tray
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(tray);
            }

            // 收养的外部内核不走状态回调：这里补挂托盘速率与节点菜单数据
            sync_tray_traffic(app.handle());
            // 收养不走状态回调：日志落盘订阅同样需要补挂
            update_logs_persistence(app.handle());
            spawn_logs_sweeper(app.handle());
            // 更新检查线程（启动 30s 后首查，之后每 6h）
            update_check::spawn_checker(app.handle());
            activity_reporting::spawn(app.handle());
            #[cfg(target_os = "macos")]
            refresh_tray_nodes(app.handle(), false);

            #[cfg(target_os = "macos")]
            if let Err(error) = crate::lifecycle::macos_shutdown::install(app.handle()) {
                // 增强入口：Dock/注销退出委托装不上时不阻塞启动，
                // 退出恢复仍由 Tauri 的 ExitRequested 路径覆盖。
                eprintln!("[crossbow] {error}");
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "notifications" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    notifications::close_overlay(window.app_handle());
                }
            }
            if matches!(event, tauri::WindowEvent::Destroyed) {
                notifications::window_destroyed(window.app_handle(), window.label());
            }
            if window.label() != "main" {
                return;
            }
            #[cfg(windows)]
            if let tauri::WindowEvent::ThemeChanged(theme) = event {
                let follows_system =
                    window
                        .app_handle()
                        .try_state::<AppState>()
                        .is_some_and(|state| {
                            state
                                .store
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .data()
                                .ui
                                .theme
                                == "system"
                        });
                if follows_system {
                    let dark = *theme == tauri::Theme::Dark;
                    if let Some(main) = window.app_handle().get_webview_window("main") {
                        crate::platform::windows_appearance::apply(&main, dark);
                        let (r, g, b) = if dark { (10, 12, 16) } else { (246, 247, 249) };
                        let _ = main
                            .set_background_color(Some(tauri::utils::config::Color(r, g, b, 255)));
                    }
                }
            }
            // 关窗：默认驻留托盘；开启「轻量模式」时交接退出（留核）。
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                let lightweight = app
                    .try_state::<AppState>()
                    .map(|s| {
                        s.store
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .data()
                            .ui
                            .lightweight_close
                    })
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
            core_restart,
            core_stop,
            core_status,
            sysproxy_status,
            sysproxy_toggle,
            diagnosis_enable_sysproxy,
            core_mode,
            set_core_mode,
            subscribe_traffic,
            unsubscribe_traffic,
            traffic_history,
            subscribe_connections,
            unsubscribe_connections,
            subscribe_logs,
            clear_logs,
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
            set_deep_link_import_confirm,
            get_activity_reporting,
            set_activity_reporting,
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
            set_linux_wireguard_coexist,
            diag_log,
            run_diagnosis,
            open_diagnosis_guide,
            workflow::profile_statuses,
            workflow::config_snapshots,
            workflow::restore_config_snapshot,
            workflow::redact_log_lines,
            workflow::export_log_lines,
            test_site,
            current_outbound,
            close_connection,
            proxies_snapshot,
            select_proxy,
            reset_proxy_selection,
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
                    if let Err(error) = crate::tray::icon::install() {
                        // Cosmetic fallback: keep the system icon, never fail startup.
                        eprintln!("[crossbow] {error}");
                    }
                }
                // 轻量待机：最后一个窗口销毁会触发 ExitRequested，
                // handoff 时阻止退出（进程保留、托盘常驻、内核照跑）。
                // swap 一次性消费：只拦截「轻量关窗」这一次请求，用完即复位，
                // 否则粘性标志会连后续 cmd-Q / quit 事件一起拦死，进程永远杀不掉。
                tauri::RunEvent::ExitRequested { code, api, .. } => {
                    if let Some(state) = app.try_state::<AppState>() {
                        if code.is_none()
                            && state
                                .handoff
                                .swap(false, std::sync::atomic::Ordering::SeqCst)
                        {
                            api.prevent_exit();
                        } else if !state.exit.ready() {
                            api.prevent_exit();
                            request_process_exit(app, code);
                        }
                    }
                }
                // macOS：Spotlight/访达/Dock 对已运行应用再次「打开」不会产生
                // 第二个进程，LaunchServices 只激活现有实例并投递 reopen 事件
                // （single-instance 插件拦截的是真第二进程，macOS 上沾不上边）。
                // 轻量待机中主窗口已销毁，必须在这里重建/呈现，否则 Dock 图标
                // 弹一下就没了下文（与托盘「打开主界面」同一条路径）。
                #[cfg(target_os = "macos")]
                tauri::RunEvent::Reopen { .. } => {
                    show_main_window(app);
                }
                // 所有可失败的网络恢复都已在 ExitRequested 前置阶段完成。
                tauri::RunEvent::Exit => {
                    #[cfg(target_os = "linux")]
                    notifications::shutdown(app);
                    if let Some(state) = app.try_state::<AppState>() {
                        // Tauri can launch the replacement process immediately
                        // after this callback returns, before this process exits.
                        state
                            .instance_lock
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .take();
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
    fn instance_lock_child() {
        // The parent starts this test in a separate process to exercise
        // operating system lock release without running any destructors.
        if let Some(directory) = std::env::var_os("CROSSBOW_TEST_INSTANCE_DIR") {
            let _lock = acquire_app_instance(std::path::Path::new(&directory)).unwrap();
            std::process::exit(0);
        }
    }

    #[test]
    fn only_one_startup_recovers_a_crashed_instances_pidfile() {
        let directory = tempfile::tempdir().unwrap();
        let pidfile = directory.path().join("app.pid");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::instance_lock_child"])
            .env("CROSSBOW_TEST_INSTANCE_DIR", directory.path())
            .spawn()
            .unwrap();
        assert!(child.wait().unwrap().success());
        assert_eq!(
            std::fs::read_to_string(&pidfile).unwrap(),
            child.id().to_string()
        );

        let attempts = 8;
        let barrier = std::sync::Barrier::new(attempts);
        std::thread::scope(|scope| {
            let workers: Vec<_> = (0..attempts)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        acquire_app_instance(directory.path())
                    })
                })
                .collect();
            // Keep every successful handle alive until every attempt has returned.
            let results: Vec<_> = workers
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .collect();
            assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
            for error in results.iter().filter_map(|result| result.as_ref().err()) {
                assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
            }
            assert_eq!(
                std::fs::read_to_string(&pidfile).unwrap(),
                std::process::id().to_string()
            );
        });
    }

    #[test]
    fn app_instance_lock_honors_legacy_pid_and_recovers_partial_writes() {
        let directory = tempfile::tempdir().unwrap();
        let pidfile = directory.path().join("app.pid");
        std::fs::write(&pidfile, std::process::id().to_string()).unwrap();
        let error = acquire_app_instance(directory.path()).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        assert!(error
            .to_string()
            .contains(&format!("(pid {})", std::process::id())));
        for content in [b"".as_slice(), b"not a pid", b"0", b"4294967296"] {
            // A parallel fork can briefly inherit a dropped flock until exec on macOS.
            let directory = tempfile::tempdir().unwrap();
            let pidfile = directory.path().join("app.pid");
            std::fs::write(&pidfile, content).unwrap();
            let _lock = acquire_app_instance(directory.path()).unwrap();
            assert_eq!(
                std::fs::read_to_string(&pidfile).unwrap(),
                std::process::id().to_string()
            );
        }
    }

    #[test]
    fn deep_link_target_must_be_http_and_display_only_its_origin() {
        let target = deep_link_import_target(
            "crossbow://import?url=https%3A%2F%2Fuser%3Asecret%40example.com%3A8443%2Fprivate%3Ftoken%3Dsecret%23fragment",
        )
        .unwrap();
        assert_eq!(
            target.origin().ascii_serialization(),
            "https://example.com:8443"
        );
        assert!(deep_link_import_target("https://import?url=https://example.com").is_none());
        assert!(deep_link_import_target("crossbow://import?url=file:///etc/hosts").is_none());
        assert!(deep_link_import_target("crossbow://other?url=https://example.com").is_none());
        assert!(deep_link_import_target("crossbow://import").is_none());
    }

    #[test]
    fn deep_link_import_rejects_cancel_and_late_confirmation() {
        assert!(!deep_link_confirmation_accepted(false, Duration::ZERO));
        assert!(deep_link_confirmation_accepted(
            true,
            DEEP_LINK_CONFIRM_TIMEOUT - Duration::from_millis(1),
        ));
        assert!(!deep_link_confirmation_accepted(
            true,
            DEEP_LINK_CONFIRM_TIMEOUT
        ));
    }

    #[test]
    fn deep_link_import_allows_one_active_request() {
        let active = AtomicBool::new(false);
        let guard = DeepLinkImportGuard::acquire(&active).unwrap();
        assert!(DeepLinkImportGuard::acquire(&active).is_none());
        assert!(DeepLinkImportGuard::acquire(&active).is_none());
        drop(guard);
        assert!(DeepLinkImportGuard::acquire(&active).is_some());
    }

    #[test]
    fn stale_proxy_clear_truth_table() {
        // journal 记录了端口且无监听 → 清理
        assert!(stale_proxy_should_clear(Some(7898), false));
        // 端口有监听（内核被收养/其他客户端占用）→ 不动
        assert!(!stale_proxy_should_clear(Some(7898), true));
        // journal 无记录（port 0 / 文件缺失）→ 不动
        assert!(!stale_proxy_should_clear(Some(0), false));
        assert!(!stale_proxy_should_clear(None, false));
    }

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
        assert_eq!(
            Store::open(directory.path())
                .unwrap()
                .data()
                .engine
                .mixed_port,
            7897
        );
    }

    #[test]
    fn profile_name_from_path_strips_extension() {
        assert_eq!(profile_name_from_path("/a/b/my-config.yaml"), "my-config");
        assert_eq!(profile_name_from_path("配置.yml"), "配置");
        assert_eq!(profile_name_from_path("noext"), "noext");
        assert_eq!(profile_name_from_path("/a/b/.hidden"), ".hidden");
    }
}
