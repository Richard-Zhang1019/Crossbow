// 退出/菜单/启动期自检（#423 Stage B 从 lib.rs 提取）。
use crate::*;
use tauri::{AppHandle, Manager};
pub(crate) fn show_main_window(app: &AppHandle) {
    // Restore the regular app before rebuilding a window from Accessory mode.
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    match app.get_webview_window("main") {
        Some(win) => {
            let _ = crate::platform::window_focus::present(&win);
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
            .visible(false)
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
                    .map(|state| {
                        state
                            .store
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .data()
                            .ui
                            .theme
                            .clone()
                    })
                    .unwrap_or_else(|| "system".to_string());
                apply_window_appearance(win, &theme);
                let _ = crate::platform::window_focus::present(win);
            }
            let _ = built;
            // 内核仍是本进程的子进程（adopted/own），继续归我们管
            if let Some(state) = app.try_state::<AppState>() {
                state
                    .handoff
                    .store(false, std::sync::atomic::Ordering::SeqCst);
            }
        }
    }
}

/// 可取消的退出准备；只从工作线程调用，恢复成功前保留内核和会话资源。
pub(crate) fn cleanup_before_process_exit(
    app: &AppHandle,
    preserve_handoff: bool,
) -> Result<(), String> {
    let Some(state) = app.try_state::<AppState>() else {
        return Ok(());
    };
    if state.exit.ready() {
        return Ok(());
    }
    state.exit.begin();
    let _operation = state
        .core_op
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if state.exit.ready() {
        return Ok(());
    }
    let handed_off = preserve_handoff
        && (state.handoff.load(std::sync::atomic::Ordering::SeqCst)
            || state
                .update_handoff
                .load(std::sync::atomic::Ordering::SeqCst));
    let tun_enabled = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .engine
        .tun_enable;
    let result = state.exit.finish(
        handed_off,
        || state.sysproxy.prepare_exit(tun_enabled),
        || {
            // 用户主动退出允许完成关闭 root 内核所需的授权，拒绝则取消退出。
            if !state.core().stop_for_exit() {
                return Err("内核未能停止，已取消退出，请重试".into());
            }
            Ok(())
        },
        || {
            state.ws.unsubscribe_all();
            let _ = std::fs::remove_file(state.data_dir.join("app.pid"));
        },
    );
    if result.is_err() {
        state.sysproxy.cancel_shutdown();
    }
    result
}

/// 安装器尚未启动就先恢复网络，因为 Windows updater 的退出回调不能取消退出。
#[cfg(windows)]
pub(crate) fn prepare_windows_update_exit(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    if !state.exit.begin() {
        return Err("应用已经在准备退出".into());
    }
    let _operation = state
        .core_op
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let tun_enabled = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .engine
        .tun_enable;
    let result = state.sysproxy.prepare_exit(tun_enabled);
    if result.is_err() {
        state.exit.cancel();
    }
    result
}

/// 安装器启动失败时保留运行中的内核，恢复操作与退出重试入口。
#[cfg(windows)]
pub(crate) fn cancel_windows_update_exit(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.sysproxy.cancel_shutdown();
    state.exit.cancel();
}

/// 更新接力保留内核；没有可接力的内核时必须在不可取消的 restart 前恢复网络。
pub(crate) fn prepare_update_restart(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    if !state.exit.begin() {
        return Err("应用已经在准备退出".into());
    }
    let handed_off = {
        let _operation = state
            .core_op
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.core().prepare_handoff()
    };
    let handed_off = match handed_off {
        Ok(value) => value.is_some(),
        Err(error) => {
            state.exit.cancel();
            return Err(error);
        }
    };
    state
        .update_handoff
        .store(handed_off, std::sync::atomic::Ordering::SeqCst);
    let result = cleanup_before_process_exit(app, true);
    if result.is_err() {
        state.exit.cancel();
    }
    result
}

/// 第一阶段阻止退出，后台完成恢复；失败时回到可重试状态并展示统一通知。
pub(crate) fn request_process_exit(app: &AppHandle, code: Option<i32>) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    if !state.exit.begin() {
        return;
    }
    let handle = app.clone();
    std::thread::spawn(move || {
        let result = cleanup_before_process_exit(&handle, false);
        let callback_handle = handle.clone();
        if let Err(error) = handle.run_on_main_thread(move || {
            finish_process_exit(&callback_handle, code, result);
        }) {
            eprintln!("交付退出准备结果失败：{error}");
        }
    });
}

/// 主线程交付结果后才重新开放退出，避免旧失败结果取消新一轮原生请求。
pub(crate) fn finish_process_exit(
    handle: &AppHandle,
    code: Option<i32>,
    result: Result<(), String>,
) {
    match result {
        Ok(()) => {
            #[cfg(target_os = "macos")]
            if crate::lifecycle::macos_shutdown::complete(true) {
                return;
            }
            handle.exit(code.unwrap_or(0));
        }
        Err(error) => {
            let state = handle.state::<AppState>();
            let _ = handle.emit("sysproxy://status", state.sysproxy.status());
            refresh_tray(handle);
            show_main_window(handle);
            let mut notice = notifications::Notice::new(
                notifications::Kind::Error,
                "app.exitBlocked",
                "app-exit",
            );
            notice.details = error;
            notice.actions = vec![notifications::Action::Diagnostics];
            notifications::publish(handle, notice);
            #[cfg(target_os = "macos")]
            crate::lifecycle::macos_shutdown::complete(false);
            state.exit.cancel();
        }
    }
}

/// 系统代理总开关（托盘与首页共用）。
pub(crate) fn toggle_sysproxy(app: &AppHandle) -> Result<(), String> {
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
pub(crate) fn lightweight_quit(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.exit.ensure_open().is_err() {
        return;
    }
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
    // Page subscriptions belong to the destroyed WebView. Keep the independent
    // traffic history and log writer running while the core stays in the tray.
    for channel in ["traffic", "connections", "logs"] {
        state.ws.unsubscribe(channel);
    }
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.destroy(); // 直接销毁，绕过 CloseRequested 防递归
    }
    // Dock 图标隐藏（仅剩顶部状态栏托盘）；cmd+tab 切换能力保留
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
}

pub(crate) fn show_no_more(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.hide();
    }
}

pub(crate) fn show_tray_toggle_error(app: &AppHandle, source: &str, code: &str, error: String) {
    notifications::result(app, source, code, Err(error));
}

pub(crate) fn handle_menu(app: &AppHandle, id: &str) {
    let result = match id {
        "toggle-proxy" => {
            let handle = app.clone();
            std::thread::spawn(move || {
                notifications::publish(
                    &handle,
                    notifications::Notice::new(
                        notifications::Kind::Progress,
                        "tray.proxy",
                        "tray-proxy",
                    ),
                );
                let result = toggle_sysproxy(&handle);
                if let Err(error) = &result {
                    eprintln!("tray system proxy toggle failed: {error}");
                }
                publish_proxy_toggle_result(&handle, "tray-proxy", result);
            });
            Ok(())
        }
        "toggle-tun" => {
            #[cfg(target_os = "windows")]
            {
                let handle = app.clone();
                std::thread::spawn(move || {
                    notifications::publish(
                        &handle,
                        notifications::Notice::new(
                            notifications::Kind::Progress,
                            "tray.tun",
                            "tray-tun",
                        ),
                    );
                    let state = handle.state::<AppState>();
                    let enabled = state
                        .store
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .data()
                        .engine
                        .tun_enable;
                    let result = crate::network::tun_windows::toggle_tun(&state, !enabled);
                    let actual = state
                        .store
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .data()
                        .engine
                        .tun_enable;
                    let _ = handle.emit("tun://status", actual);
                    let _ = handle.emit("sysproxy://status", state.sysproxy.status());
                    refresh_tray(&handle);
                    if let Err(error) = result {
                        eprintln!("tray TUN toggle failed: {error}");
                        show_tray_toggle_error(&handle, "tray-tun", "tray.tun", error);
                    } else {
                        notifications::result(&handle, "tray-tun", "tray.tun", Ok(()));
                    }
                });
            }
            #[cfg(target_os = "macos")]
            {
                let handle = app.clone();
                std::thread::spawn(move || {
                    notifications::publish(
                        &handle,
                        notifications::Notice::new(
                            notifications::Kind::Progress,
                            "tray.tun",
                            "tray-tun",
                        ),
                    );
                    let state = handle.state::<AppState>();
                    let enabled = state
                        .store
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .data()
                        .engine
                        .tun_enable;
                    let result = crate::network::tun::toggle_tun(&state, !enabled);
                    let actual = state
                        .store
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .data()
                        .engine
                        .tun_enable;
                    let _ = handle.emit("tun://status", actual);
                    let _ = handle.emit("sysproxy://status", state.sysproxy.status());
                    refresh_tray(&handle);
                    if let Err(error) = result {
                        eprintln!("tray TUN toggle failed: {error}");
                        show_tray_toggle_error(&handle, "tray-tun", "tray.tun", error);
                    } else {
                        notifications::result(&handle, "tray-tun", "tray.tun", Ok(()));
                    }
                });
            }
            Ok(())
        }
        "mode-direct" | "mode-rule" | "mode-global" => {
            let mode = id
                .strip_prefix("mode-")
                .expect("上方 starts_with 已保证 mode- 前缀")
                .to_string();
            let handle = app.clone();
            std::thread::spawn(move || {
                notifications::publish(
                    &handle,
                    notifications::Notice::new(
                        notifications::Kind::Progress,
                        "core.mode",
                        "tray-mode",
                    ),
                );
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
                    .unwrap_or_else(|e| e.into_inner())
                    .group()
                    .to_string();
                let h = app.clone();
                let name = name.to_string();
                std::thread::spawn(move || {
                    let result = select_proxy_inner(&h.state::<AppState>(), &group, &name);
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
                    notifications::publish(
                        &handle,
                        notifications::Notice::new(
                            notifications::Kind::Progress,
                            "tray.profile",
                            "tray-profile",
                        ),
                    );
                    let result = tauri::async_runtime::block_on(set_active_profile(
                        handle.clone(),
                        handle.state::<AppState>(),
                        profile,
                    ));
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
pub(crate) fn show_fatal(msg: &str) {
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
pub(crate) fn deelevate_if_needed() {
    const SKIP_ENV: &str = "CROSSBOW_DEELEVATED";
    if std::env::var_os(SKIP_ENV).is_some() || !is_elevated() {
        return;
    }
    // explorer.exe 转交启动请求给常驻 shell 时环境变量不会传给目标进程
    // （UAC 关闭的机器上目标仍带完整管理员令牌），单靠 env 防循环会无限
    // 重启（#325）。改用 ProgramData 标记 + 时间窗兜底：跨账号、跨 shell
    // 交接都稳定；120s 过期避免误伤后续手动提权运行。
    let marker = deelevate_marker_path();
    if let Some(dir) = marker.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let fresh = std::fs::metadata(&marker)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.elapsed().ok())
        .map(|elapsed| elapsed < std::time::Duration::from_secs(120))
        .unwrap_or(false);
    if fresh {
        // 刚降权过仍被提权（UAC 关闭环境）：不再循环，直接运行。
        let _ = std::fs::remove_file(&marker);
        eprintln!("elevated again within deelevate window; continuing as-is");
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    if std::fs::write(&marker, b"1").is_err() {
        return; // 标记写不进去就无法防循环，宁可直接提权运行
    }
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
    // explorer 派生失败（shell 未运行等）→ 回滚标记，继续提权运行，可用性优先。
    let _ = std::fs::remove_file(&marker);
    eprintln!("deelevate relaunch failed; continuing elevated");
}

#[cfg(target_os = "windows")]
pub(crate) fn deelevate_marker_path() -> std::path::PathBuf {
    // %ProgramData%（跨账号共享可写）；环境变量缺失时回退默认路径。
    std::env::var_os("ProgramData")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("C:\\ProgramData"))
        .join("Crossbow")
        .join("deelevate.marker")
}

#[cfg(target_os = "windows")]
pub(crate) fn is_elevated() -> bool {
    // windows-sys 0.59 未导出 OpenProcessToken；IsUserAnAdmin（shell32）判定
    // 当前令牌是否处于管理员组——UAC 拆分令牌（未提权）会正确返回 false。
    unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0 }
}

/// panic 落盘（#352）：GUI 进程 stderr 不可见，启动路径的 panic（如
/// 昨日 did_finish_launching SIGABRT ×3）过去完全无消息留痕。在
/// Builder 之前注册——覆盖 setup 阶段的所有 panic；写数据目录
/// panic.log（时间 + 线程 + 消息 + 位置），写失败静默放弃（钩子里
/// 不能再 panic）。
pub(crate) fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".into());
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "<non-string panic payload>".into()
        };
        let thread = std::thread::current();
        let thread_name = thread.name().unwrap_or("<unnamed>").to_string();
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let line = format!("[{ms}] thread '{thread_name}' panicked at {location}: {payload}\n");
        // 与 data_dir 同目标但不依赖 AppState：macOS ~/Library/Application
        // Support；Windows %APPDATA%。写失败静默放弃（钩子内不能再 panic）。
        #[cfg(target_os = "macos")]
        let dir = std::env::var_os("HOME")
            .map(|h| std::path::PathBuf::from(h))
            .map(|h| h.join("Library/Application Support/com.richard.crossbow"));
        #[cfg(target_os = "windows")]
        let dir = std::env::var_os("APPDATA")
            .map(|h| std::path::PathBuf::from(h))
            .map(|h| h.join("com.richard.crossbow"));
        #[cfg(not(any(target_os = "macos", windows)))]
        let dir: Option<std::path::PathBuf> = None;
        if let Some(dir) = dir {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("panic.log"))
                .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
        }
        default_hook(info);
    }));
}

/// 启动最前部的残留系统代理自愈：上一实例崩溃（如 did_finish_launching
/// panic，早于 setup 内的 recover_stale）会把系统代理留在死端口上，走代理
/// 的应用整机断网。journal 记录了 Crossbow 曾开启代理且该端口现在无监听
/// → 还原系统代理（按快照）。在 Builder 之前执行，任何后续 panic 都不再
/// 影响网络可用性。端口有监听（内核被收养/别的客户端占用同端口）则不动。
#[cfg(any(target_os = "macos", windows))]
pub(crate) fn clear_stale_system_proxy_early() {
    let dir = data_dir_for_early_hooks();
    let journal = dir.join("sysproxy-journal.json");
    if !journal.is_file() {
        return;
    }
    // Recovery also changes shared state. Skip it if another instance is
    // already starting or owns the desktop session.
    let Ok(_lock) = lock_app_instance(&dir) else {
        return;
    };
    let Ok(raw) = std::fs::read_to_string(&journal) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return;
    };
    let port = value.get("port").and_then(|p| p.as_u64()).unwrap_or(0) as u16;
    if !stale_proxy_should_clear(Some(port), port_has_listener(port)) {
        return;
    }
    eprintln!("stale system proxy: journal port {port} has no listener; restoring settings");
    #[cfg(target_os = "macos")]
    let backend = crate::network::sysproxy::NetworkSetup;
    #[cfg(windows)]
    let backend = crate::network::sysproxy::WinInet;
    let manager = crate::network::sysproxy::SysProxyManager::new(backend, journal);
    if let Err(error) = manager.disable() {
        eprintln!("stale system proxy restore failed: {error}");
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
pub(crate) fn clear_stale_system_proxy_early() {}

/// 自愈判定（纯函数便于测试）：journal 记录了非零端口且该端口无监听。
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(crate) fn stale_proxy_should_clear(journal_port: Option<u16>, port_listening: bool) -> bool {
    journal_port.is_some_and(|p| p > 0) && !port_listening
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(crate) fn port_has_listener(port: u16) -> bool {
    use std::net::{Ipv4Addr, SocketAddr, TcpStream};
    use std::time::Duration;
    TcpStream::connect_timeout(
        &SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        Duration::from_millis(300),
    )
    .is_ok()
}

#[cfg(any(target_os = "macos", windows))]
pub(crate) fn data_dir_for_early_hooks() -> std::path::PathBuf {
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .map(|h| h.join("Library/Application Support/com.richard.crossbow"))
            .expect("HOME unset; cannot locate data dir")
    }
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA")
            .map(std::path::PathBuf::from)
            .map(|h| h.join("com.richard.crossbow"))
            .expect("APPDATA unset; cannot locate data dir")
    }
}
