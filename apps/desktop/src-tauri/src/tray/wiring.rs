// 托盘接线与页面通道（#423 Stage B 从 lib.rs 提取）。
use crate::engine::timing;
use crate::*;
use tauri::{AppHandle, Emitter, Manager};

/// 系统代理端口恢复重试间隔（等待端口释放）。
const SYS_PROXY_RETRY_WAIT: std::time::Duration = std::time::Duration::from_millis(200);
const SYS_PROXY_RECOVERY_SOURCE: &str = "sysproxy-core-recovery";
/// 日志清扫器：尚无应用状态时的空转等待。
const LOGS_SWEEP_IDLE_WAIT: std::time::Duration = std::time::Duration::from_secs(600);
/// 日志清扫器：常规清扫周期。
const LOGS_SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(6 * 3600);
/// TUN 真实连通性探测超时（gstatic/github 204 探测）。
#[cfg(any(target_os = "macos", windows))]
const TUN_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// 托盘批量测速并发上限，也是看门狗计算每批节点数的依据。
#[cfg(target_os = "macos")]
const SPEED_TEST_CONCURRENCY: usize = 10;
/// 托盘批量测速看门狗基础余量。
#[cfg(target_os = "macos")]
const SPEED_TEST_WATCHDOG_BASE: Duration = Duration::from_secs(20);
/// 每批测速的请求预算。
#[cfg(target_os = "macos")]
const SPEED_TEST_WATCHDOG_PER_BATCH_SECS: u64 = 6;

fn sysproxy_recovery_failure_notice(error: String) -> notifications::Notice {
    let mut notice = notifications::Notice::new(
        notifications::Kind::Error,
        "sysproxy.restoreFailed",
        SYS_PROXY_RECOVERY_SOURCE,
    );
    notice.details = error;
    notice.actions = vec![
        notifications::Action::RetrySystemProxy,
        notifications::Action::Diagnostics,
    ];
    notice
}

pub(crate) fn core_status_callback(handle: AppHandle) -> crate::engine::manager::Callback {
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
        // 最近日志缓存始终跟随内核；落盘另由保存开关控制。
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
            if crate::network::linux_service::status()
                .is_ok_and(|session| session.is_none_or(|s| !s.active))
            {
                let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
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
                match state.sysproxy.disable() {
                    Ok(()) => notifications::resolve(&handle, SYS_PROXY_RECOVERY_SOURCE),
                    Err(error) => {
                        eprintln!("core crash system proxy recovery failed: {error}");
                        notifications::publish(&handle, sysproxy_recovery_failure_notice(error));
                    }
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
                    let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
                    for o in store.data_mut().overrides.iter_mut() {
                        o.enabled = false;
                    }
                    let _ = store.save();
                }
                let (port, tun_enabled) = {
                    let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
                    (
                        store.data().engine.mixed_port,
                        store.data().engine.tun_enable,
                    )
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
                        std::thread::sleep(SYS_PROXY_RETRY_WAIT);
                    }
                }
                let mut notice = notifications::Notice::new(
                    notifications::Kind::Warning,
                    "core.safeMode",
                    "core-safe-mode",
                );
                notice.duration = Some(0);
                notice.actions = vec![notifications::Action::Profiles];
                notifications::publish(&handle, notice);
                refresh_tray(&handle);
            }
        }
    })
}

pub(crate) fn sync_tray_traffic(handle: &AppHandle) {
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
                    crate::tray::speed::set_speed(&h, up, down);
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
    crate::tray::speed::clear(handle);
}

/// 内核（重）启后重订仍被页面需要的通道（traffic/connections/logs）。
/// 页面订阅绑定挂载时的控制器地址——内核一重启端口/密钥全换，旧读线程
/// 会在死端口上无限重连，日志页空、首页图表与连接页停摆都是这个缺口。
/// 借 subscribe 的替换语义，这里用新地址顶替旧通道；意图集（页面挂载
/// 即登记、卸载即移除）保证 Starting 期间挂载失败的页面 Running 后补上。
pub(crate) fn resubscribe_page_channels(app: &AppHandle) {
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
    let _ = state.ws.subscribe_traffic(port, &secret, emit.clone());
    let _ = state.ws.subscribe_connections(port, &secret, emit.clone());
    let _ = state.ws.subscribe_logs(port, &secret, emit);
}

/// 页面通道冻结看门狗：内核 Running 但页面 traffic/connections 通道静默
/// 超阈值——典型成因是重启竞态绕过了 Running 补订，旧读线程在换掉的
/// port/secret（死端口）上无限重连——此时用当前控制器地址强制重订。
/// 与托盘通道看门狗同构（ensure_tray_speed_watchdog）；判定逻辑见
/// WsHub::any_page_channel_stale（logs 事件驱动不参与）。#111
pub(crate) fn ensure_page_channel_watchdog(handle: &AppHandle) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static SPAWNED: AtomicBool = AtomicBool::new(false);
    if SPAWNED.swap(true, Ordering::SeqCst) {
        return;
    }
    let h = handle.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(timing::WATCHDOG_TICK);
        let Some(state) = h.try_state::<AppState>() else {
            continue;
        };
        if state.core().status() != CoreStatus::Running {
            continue;
        }
        if state.ws.any_page_channel_stale(timing::PAGE_CHANNEL_STALE) {
            eprintln!(
                "page watchdog: traffic/connections silent >={}s, resubscribing",
                timing::PAGE_CHANNEL_STALE.as_secs()
            );
            resubscribe_page_channels(&h);
        }
    });
}

/// 内存最近日志始终跟随内核；落盘通道另由保存开关控制（#29）。
/// 设置开关变化与状态回调都会调用，缓存 reader 对同一控制器保持幂等。
pub(crate) fn update_logs_persistence(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    if state.core().status() == CoreStatus::Running {
        if let Some((port, secret)) = state.core().controller() {
            let h = app.clone();
            let _ = state
                .ws
                .subscribe_logs(port, &secret, move |event, payload| {
                    let _ = h.emit(event, payload);
                });
        }
    } else {
        state.ws.stop_logs();
    }
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
    let mut writer = crate::diagnostics::log_files::DailyWriter::new(
        crate::diagnostics::log_files::logs_dir(&state.data_dir),
    );
    let _ = state
        .ws
        .subscribe_logs_file(port, &secret, move |level, payload| {
            writer.write(level, payload);
        });
}

/// 保留期清理：启动时与每 6 小时各跑一次（按当前设置的天数）。
/// 一次性迁移（#28 标识符切换 com.crossbow.app → com.richard.crossbow）：
/// 新数据目录不存在时整体改名（原子、保留 setuid 与全部内容）；
/// 已存在（Tauri 先建了空目录）则把旧目录里缺的条目逐个移入。
pub(crate) fn migrate_legacy_data_dir(new_dir: &std::path::Path) {
    let Some(parent) = new_dir.parent() else {
        return;
    };
    let legacy = parent.join("com.crossbow.app");
    if !legacy.is_dir() || new_dir == legacy {
        return;
    }
    if !new_dir.exists() && std::fs::rename(&legacy, new_dir).is_ok() {
        eprintln!("migrated legacy data dir -> {}", new_dir.display());
        return;
    }
    let Ok(entries) = std::fs::read_dir(&legacy) else {
        return;
    };
    for entry in entries.flatten() {
        let target = new_dir.join(entry.file_name());
        if !target.exists() {
            let _ = std::fs::rename(entry.path(), &target);
        }
    }
}

pub(crate) fn spawn_logs_sweeper(app: &AppHandle) {
    let h = app.clone();
    std::thread::spawn(move || loop {
        let Some(state) = h.try_state::<AppState>() else {
            std::thread::sleep(LOGS_SWEEP_IDLE_WAIT);
            continue;
        };
        let days = state
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .data()
            .logs
            .retention_days;
        let dir = crate::diagnostics::log_files::logs_dir(&state.data_dir);
        crate::diagnostics::log_files::sweep(&dir, days);
        std::thread::sleep(LOGS_SWEEP_INTERVAL);
    });
}

/// Probe actual traffic through the default route. A node-delay benchmark can
/// fail even while normal websites work, so it must not disable a healthy TUN.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) fn probe_tun_network() -> Result<(), String> {
    let agent = ureq::AgentBuilder::new()
        .try_proxy_from_env(false)
        .timeout(TUN_PROBE_TIMEOUT)
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

/// #481 直连基准探测的国内外目标：国内 204 端点判定物理网络是否中断
/// （国外直连被墙是常态，不能作为断网依据）；国外端点用于区分「机场故障」。
const BASELINE_PROBE_CN: &str = "http://connect.rom.miui.com/generate_204";
const BASELINE_PROBE_GLOBAL: &str = "http://www.gstatic.com/generate_204";
/// 同一故障事件的提示频控窗口：期间反复测速不重复弹通知。
const BASELINE_REPORT_INTERVAL: Duration = Duration::from_secs(10 * 60);

/// #481 判定矩阵（纯函数，便于单测）：返回 (通知 code, 严重度)。
/// 国内直连失败是「本机断网」的充分信号——此时节点是否可达无从谈起；
/// 国内可达而国外失败属常态（墙），配合测速全超时应提示机场/节点故障。
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn baseline_verdict(cn: bool, global: bool) -> (&'static str, notifications::Kind) {
    match (cn, global) {
        (false, _) => ("tray.networkDown", notifications::Kind::Warning),
        (true, false) => ("tray.nodesDown", notifications::Kind::Warning),
        (true, true) => ("tray.speedTestAnomaly", notifications::Kind::Info),
    }
}

/// 单目标探测：任何 HTTP 响应（含 4xx/5xx）都证明 TCP 路径可达——判定
/// 只关心链路通断，不关心服务状态码。
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn baseline_probe_one(agent: &ureq::Agent, url: &str) -> bool {
    match agent.head(url).call() {
        Ok(_) | Err(ureq::Error::Status(_, _)) => true,
        Err(_) => false,
    }
}

/// 托盘测速全部失败后的直连基准探测：只在探测完成后按判定矩阵发一次
/// 通知（10 分钟频控），不切节点、不改任何状态——纯甄别提示，避免把
/// 本机断网误归因为代理故障（2026-11-13 事件复盘，#481）。
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn maybe_report_baseline(handle: &AppHandle) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST_REPORT_MS: AtomicU64 = AtomicU64::new(0);

    let h = handle.clone();
    std::thread::spawn(move || {
        let agent = ureq::AgentBuilder::new()
            .try_proxy_from_env(false)
            .timeout(TUN_PROBE_TIMEOUT)
            .build();
        // 国内目标优先：断网时它先失败，国外探测不必再等一个完整超时。
        let cn = baseline_probe_one(&agent, BASELINE_PROBE_CN);
        let global = if cn {
            baseline_probe_one(&agent, BASELINE_PROBE_GLOBAL)
        } else {
            false
        };
        let (code, kind) = baseline_verdict(cn, global);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if now_ms.saturating_sub(LAST_REPORT_MS.load(Ordering::Relaxed))
            < BASELINE_REPORT_INTERVAL.as_millis() as u64
        {
            return;
        }
        LAST_REPORT_MS.store(now_ms, Ordering::Relaxed);
        let notice = notifications::Notice::new(kind, code, "tray-baseline");
        notifications::publish(&h, notice);
    });
}

/// 健康检查重建立。与手动开关共用 core_op，进锁后再读 TUN 标志。
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) fn reestablish_tun_if_enabled(state: &AppState) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        crate::network::tun::reestablish_if_enabled(state)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = state;
        Ok(())
    }
}

#[cfg(any(target_os = "macos", windows))]
pub(crate) enum HealthRecovery {
    Superseded,
    Restarted,
    Disabled,
}

#[cfg(target_os = "macos")]
fn recover_for_health(
    state: &AppState,
    expected_pid: Option<u32>,
    expected_status: &CoreStatus,
    may_restart: bool,
) -> Result<HealthRecovery, String> {
    let _operation = state.core_op.lock().unwrap_or_else(|e| e.into_inner());
    state.exit.ensure_open()?;
    if !state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .engine
        .tun_enable
        || state.core().pid() != expected_pid
        || &state.core().status() != expected_status
    {
        return Ok(HealthRecovery::Superseded);
    }
    if may_restart
        && *expected_status == CoreStatus::Running
        && expected_pid.is_some()
        && state.core().signal_graceful_restart()
    {
        return Ok(HealthRecovery::Restarted);
    }
    crate::network::tun::disable_for_health_locked(state)?;
    Ok(HealthRecovery::Disabled)
}

#[cfg(windows)]
use crate::network::tun_windows::recover_for_health;

#[cfg(any(target_os = "macos", windows))]
fn emit_tun_recovery_status(app: &AppHandle, state: &AppState) {
    let enabled = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .engine
        .tun_enable;
    let _ = app.emit("tun://status", enabled);
    let _ = app.emit("sysproxy://status", state.sysproxy.status());
    refresh_tray(app);
}

/// TUN 健康检查：每 5 秒检查真实网络连通性；连续 3 次失败时关闭 TUN
/// 并通知界面，避免流量持续被送入失效的隧道。
/// macOS 额外检查捕获路由，漂移时走与开关同一把锁的重建立。
#[cfg(any(target_os = "macos", windows))]
pub(crate) fn spawn_tun_health_monitor(app: &AppHandle) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static SPAWNED: AtomicBool = AtomicBool::new(false);
    if SPAWNED.swap(true, Ordering::SeqCst) {
        return;
    }

    let handle = app.clone();
    std::thread::spawn(move || {
        let _background = crate::lifecycle::authorization::BackgroundGuard::enter();
        let mut failures = 0u8;
        let mut drift_strikes = 0u8;
        let mut reestablish_count = 0u8;
        let mut reestablish_cooldown_until = std::time::Instant::now();
        let mut reported_giveup = false;
        let mut reported_recovery_failure = false;
        let mut external_proxy_notice = crate::network::sysproxy::ExternalProxyNotice::default();
        // TUN 栈楔死（#350）：保留重启预算与冷却，以及系统代理死亡看门狗。
        let mut wedge_restarts = 0u8;
        let mut wedge_cooldown_until = std::time::Instant::now();
        let mut sysproxy_dead_ticks = 0u8;
        let mut last_probe_pid = None;
        loop {
            let cycle_started = std::time::Instant::now();
            if let Some(state) = handle.try_state::<AppState>() {
                let tun_enabled = state
                    .store
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .data()
                    .engine
                    .tun_enable;
                let core_status = state.core().status();
                let core_pid = state.core().pid();
                if last_probe_pid != core_pid {
                    failures = 0;
                    last_probe_pid = core_pid;
                }
                if !tun_enabled
                    || core_status == CoreStatus::Starting
                    || state.exit.ensure_open().is_err()
                {
                    if !tun_enabled {
                        wedge_restarts = 0;
                        wedge_cooldown_until = cycle_started;
                    }
                    if !tun_enabled && external_proxy_notice.update(false) == Some(false) {
                        notifications::resolve(&handle, "tun-external-proxy");
                    }
                    // 系统代理模式（无 TUN）的兜底：Crossbow 开着系统代理而内核
                    // 死亡 → 代理指向死端口，走代理的应用整机断网。连续 3 个周期
                    // （15s）核心未恢复 → 关闭系统代理（还原快照），直连流量立即
                    // 恢复，避免黑洞。仅清 Crossbow 自己开的代理（status() 读
                    // 内存状态，不碰其他客户端设置的代理）。
                    let sysproxy_mode = state.exit.ensure_open().is_ok()
                        && !tun_enabled
                        && state.sysproxy.status().enabled
                        && matches!(core_status, CoreStatus::Crashed(_) | CoreStatus::Stopped);
                    if sysproxy_mode {
                        sysproxy_dead_ticks = sysproxy_dead_ticks.saturating_add(1);
                        if sysproxy_dead_ticks >= timing::FAILURE_BUDGET {
                            sysproxy_dead_ticks = 0;
                            let _operation =
                                state.core_op.lock().unwrap_or_else(|e| e.into_inner());
                            if state.exit.ensure_open().is_ok()
                                && !state
                                    .store
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .data()
                                    .engine
                                    .tun_enable
                                && matches!(
                                    state.core().status(),
                                    CoreStatus::Crashed(_) | CoreStatus::Stopped
                                )
                            {
                                match state.sysproxy.disable() {
                                    Ok(()) => {
                                        let _ = handle
                                            .emit("sysproxy://status", state.sysproxy.status());
                                        refresh_tray(&handle);
                                        let mut notice = notifications::Notice::new(
                                            notifications::Kind::Warning,
                                            "sysproxy.coreDead",
                                            "tun-health",
                                        );
                                        notice.duration = Some(0);
                                        notifications::publish(&handle, notice);
                                        notifications::resolve(&handle, SYS_PROXY_RECOVERY_SOURCE);
                                    }
                                    Err(error) => {
                                        eprintln!("sysproxy stale-off failed: {error}");
                                        let _ = handle
                                            .emit("sysproxy://status", state.sysproxy.status());
                                        notifications::publish(
                                            &handle,
                                            sysproxy_recovery_failure_notice(error),
                                        );
                                    }
                                }
                            }
                        }
                    } else {
                        sysproxy_dead_ticks = 0;
                    }
                    failures = 0;
                    drift_strikes = 0;
                    reestablish_count = 0;
                    reported_giveup = false;
                    reported_recovery_failure = false;
                } else {
                    // 接管漂移（#154）：捕获路由丢失后，连通性探测可能假绿
                    // （gstatic 存在国内可达地址），显式检查接管本体。漂移
                    // 持续即关开重建立（2 分钟冷却、连续 3 次后停止自动恢复
                    // 并一次性通知），重建立周期跳过连通性探测。
                    let mut skip_probe = false;
                    let wedge_cooling = std::time::Instant::now() < wedge_cooldown_until;
                    if wedge_cooling {
                        // 重启内核后的稳定期：跳过探测与漂移检测，失败计数清零
                        skip_probe = true;
                        failures = 0;
                        drift_strikes = 0;
                    }
                    let cooling = std::time::Instant::now() < reestablish_cooldown_until;
                    #[cfg(target_os = "macos")]
                    let drifted =
                        !wedge_cooling && !cooling && crate::network::tun::takeover_drift();
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
                            reestablish_cooldown_until =
                                std::time::Instant::now() + timing::REESTABLISH_COOLDOWN;
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
                                        let _ = handle
                                            .emit("sysproxy://status", state.sysproxy.status());
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
                                    eprintln!("tun takeover re-establish failed: {error}");
                                    failures = failures.saturating_add(1);
                                    if !reported_recovery_failure {
                                        reported_recovery_failure = true;
                                        let mut notice = notifications::Notice::new(
                                            notifications::Kind::Warning,
                                            "tun.manualRecovery",
                                            "tun-health",
                                        );
                                        notice.details = error;
                                        notice.duration = Some(0);
                                        notice.actions = vec![
                                            notifications::Action::Settings,
                                            notifications::Action::Diagnostics,
                                        ];
                                        notifications::publish(&handle, notice);
                                    }
                                }
                            }
                            skip_probe = true;
                        }
                    }

                    let probe = if wedge_cooling {
                        Ok(())
                    } else if core_status == CoreStatus::Running {
                        // A second proxy client may re-enable a stale macOS
                        // proxy after Crossbow has established its route.
                        let conflict = crate::network::sysproxy::check_external_proxy_for_tun(
                            &state.core_op,
                            || {
                                state.exit.ensure_open().is_ok()
                                    && state
                                        .store
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner())
                                        .data()
                                        .engine
                                        .tun_enable
                            },
                            || state.sysproxy.external_proxy_conflict_for_tun(),
                        );
                        match external_proxy_notice.update(conflict) {
                            Some(true) => {
                                let mut notice = notifications::Notice::new(
                                    notifications::Kind::Warning,
                                    "tun.externalProxy",
                                    "tun-external-proxy",
                                );
                                notice.duration = Some(0);
                                notice.actions = vec![
                                    notifications::Action::Settings,
                                    notifications::Action::Diagnostics,
                                ];
                                notifications::publish(&handle, notice);
                            }
                            Some(false) => notifications::resolve(&handle, "tun-external-proxy"),
                            None => {}
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
                            if !skip_probe && !drifted && !cooling {
                                notifications::resolve(&handle, "tun-health");
                                reported_recovery_failure = false;
                                wedge_restarts = 0;
                            }
                            failures = 0;
                        }
                        Err(error) => {
                            failures = failures.saturating_add(1);
                            eprintln!(
                                "tun health check failed ({failures}/{}): {error}",
                                timing::FAILURE_BUDGET
                            );
                            if failures >= timing::FAILURE_BUDGET {
                                failures = 0;
                                let outcome = recover_for_health(
                                    &state,
                                    core_pid,
                                    &core_status,
                                    wedge_restarts < timing::WEDGE_RESTART_BUDGET,
                                );
                                match outcome {
                                    Ok(HealthRecovery::Superseded) => {}
                                    Ok(HealthRecovery::Restarted) => {
                                        wedge_restarts = wedge_restarts.saturating_add(1);
                                        wedge_cooldown_until =
                                            std::time::Instant::now() + timing::WEDGE_COOLDOWN;
                                        reported_recovery_failure = false;
                                        let mut notice = notifications::Notice::new(
                                            notifications::Kind::Warning,
                                            if cfg!(windows) {
                                                "tun.wedgeRecovered"
                                            } else {
                                                "tun.wedgeRestart"
                                            },
                                            "tun-health",
                                        );
                                        notice.duration = Some(0);
                                        notice.actions = vec![notifications::Action::Diagnostics];
                                        notifications::publish(&handle, notice);
                                    }
                                    Ok(HealthRecovery::Disabled) => {
                                        reported_recovery_failure = false;
                                        emit_tun_recovery_status(&handle, &state);
                                        let mut notice = notifications::Notice::new(
                                            notifications::Kind::Warning,
                                            "tun.disabled",
                                            "tun-health",
                                        );
                                        notice.duration = Some(0);
                                        notice.actions = vec![notifications::Action::Diagnostics];
                                        notifications::publish(&handle, notice);
                                    }
                                    Err(disable_error) => {
                                        eprintln!("tun health recovery failed: {disable_error}");
                                        emit_tun_recovery_status(&handle, &state);
                                        if !reported_recovery_failure {
                                            notifications::tun_result(
                                                &handle,
                                                &state,
                                                "tun-health",
                                                "tun.disableFailed",
                                                Err(disable_error),
                                            );
                                            reported_recovery_failure = true;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            std::thread::sleep(timing::WATCHDOG_TICK.saturating_sub(cycle_started.elapsed()));
        }
    });
}

/// 托盘速率看门狗（进程级单例）：内核 Running 期间 /traffic 静默超过阈值
/// 即强制重订。mihomo/sing-box 的 /traffic 即使零流量也每秒推帧，因此
/// 「订阅还在但静默」必然意味着读线程死亡、socket 半开或内核假活——
/// subscribe 的替换语义让重订直接顶替坏通道，冻结在数十秒内自愈。
#[cfg(target_os = "macos")]
pub(crate) fn ensure_tray_speed_watchdog(handle: &AppHandle) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static SPAWNED: AtomicBool = AtomicBool::new(false);
    if SPAWNED.swap(true, Ordering::SeqCst) {
        return;
    }
    let h = handle.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(timing::WATCHDOG_TICK);
        let Some(state) = h.try_state::<AppState>() else {
            continue;
        };
        if state.core().status() != CoreStatus::Running {
            continue;
        }
        match state.ws.last_message_ago("traffic-tray") {
            Some(silent) if silent >= timing::TRAY_SPEED_CHANNEL_STALE => {
                eprintln!(
                    "tray watchdog: traffic-tray silent {}s, resubscribing",
                    silent.as_secs()
                );
                sync_tray_traffic(&h);
            }
            _ => {}
        }
    });
}

/// 后台拉取托盘节点菜单数据：探测主选择组 + 各节点延迟，完成后刷新托盘。
#[cfg(target_os = "macos")]
pub(crate) fn refresh_tray_nodes(handle: &AppHandle, keep_menu_open: bool) {
    let Some(state) = handle.try_state::<AppState>() else {
        return;
    };
    if state.core().status() != CoreStatus::Running {
        return;
    }
    let generation = handle
        .state::<AppState>()
        .node_menu
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .generation();
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
        if tray.generation() != generation {
            return;
        }
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
        if tray.generation() != generation {
            return;
        }
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
            && crate::tray::speed::update_node_menu(&h, &data.title, &data.rows, data.testing);
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
                .filter(|r| r.ms.is_none() && r.badge != "timeout" && r.badge != "failed")
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
pub(crate) fn run_tray_delay_test(handle: &AppHandle) {
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
                for g in groups
                    .iter()
                    .filter(|g| g.kind.eq_ignore_ascii_case("URLTest"))
                {
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
            if tray.generation() == generation {
                tray.end_test();
            }
            drop(tray);
            let data = tray_menu_data(&state);
            crate::tray::speed::update_node_menu(&h, &data.title, &data.rows, data.testing);
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
            crate::tray::speed::update_node_menu(&h, &data.title, &data.rows, data.testing);
            return;
        }

        {
            let mut tray = state.node_menu.lock().unwrap_or_else(|e| e.into_inner());
            if !tray.set_test_targets(generation, &targets) {
                return;
            }
        }

        // 看门狗按并发批次估算等待上限，超时恢复可点态。
        let watchdog_handle = h.clone();
        let batches = total.div_ceil(SPEED_TEST_CONCURRENCY) as u64;
        let watchdog_timeout = SPEED_TEST_WATCHDOG_BASE
            + Duration::from_secs(batches * SPEED_TEST_WATCHDOG_PER_BATCH_SECS);
        std::thread::spawn(move || {
            std::thread::sleep(watchdog_timeout);
            let Some(state) = watchdog_handle.try_state::<AppState>() else {
                return;
            };
            let mut tray = state.node_menu.lock().unwrap_or_else(|e| e.into_inner());
            if !tray.expire_test(generation) {
                return; // 已完成或新一轮测速已接管
            }
            drop(tray);
            let data = tray_menu_data(&state);
            crate::tray::speed::update_node_menu(
                &watchdog_handle,
                &data.title,
                &data.rows,
                data.testing,
            );
        });

        let queue = std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::from(
            targets,
        )));
        let (tx, rx) = std::sync::mpsc::channel::<(String, Result<u64, String>)>();
        for _ in 0..total.min(SPEED_TEST_CONCURRENCY) {
            let queue = queue.clone();
            let tx = tx.clone();
            let h_worker = h.clone();
            std::thread::spawn(move || {
                let client = match h_worker.try_state::<AppState>() {
                    Some(s) => controller_client(&s),
                    None => return,
                };
                while let Some(node) = queue.lock().ok().and_then(|mut q| q.pop_front()) {
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
        let mut failures = 0usize;
        for (node, result) in rx {
            let Some(state) = h.try_state::<AppState>() else {
                return;
            };
            {
                let mut tray = state.node_menu.lock().unwrap_or_else(|e| e.into_inner());
                if tray.generation() != generation {
                    return; // 新一轮测速已接管，放弃本轮剩余结果
                }
                if result.is_err() {
                    failures += 1;
                }
                tray.apply_single_result(&node, result);
            }
            let data = tray_menu_data(&state);
            crate::tray::speed::update_node_menu(&h, &data.title, &data.rows, data.testing);
            done += 1;
            if done >= total {
                break;
            }
        }
        // 全部完成：恢复可点态（与看门狗竞争安全）
        let Some(state) = h.try_state::<AppState>() else {
            return;
        };
        // #481：全部节点失败 → 直连基准探测，甄别「本机断网」与「节点故障」。
        // 探测在后台线程执行（最长约两个超时窗口），不阻塞托盘恢复可点态。
        if failures >= total {
            maybe_report_baseline(&h);
        }
        let mut tray = state.node_menu.lock().unwrap_or_else(|e| e.into_inner());
        if tray.generation() != generation {
            return;
        }
        tray.end_test();
        drop(tray);
        let data = tray_menu_data(&state);
        crate::tray::speed::update_node_menu(&h, &data.title, &data.rows, data.testing);
        // URLTest may pick a different member during testing. Re-read the
        // controller selection instead of deriving it from measured delays.
        refresh_tray_nodes(&h, true);
    });
}

#[cfg(target_os = "macos")]
pub(crate) fn update_tray_node_menu(handle: &AppHandle, testing: bool) {
    let Some(state) = handle.try_state::<AppState>() else {
        return;
    };
    let lang = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .ui
        .lang
        .clone();
    let data = state
        .node_menu
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .snapshot(
            &tr(&lang, "节点选择", "Nodes"),
            &tr(&lang, "当前", "Current"),
        );
    crate::tray::speed::update_node_menu(handle, &data.title, &data.rows, testing);
}

#[cfg(target_os = "macos")]
pub(crate) fn tray_menu_data(state: &AppState) -> crate::tray::state::TrayMenuData {
    let lang = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .ui
        .lang
        .clone();
    let node_label = tr(&lang, "节点选择", "Nodes");
    let current_label = tr(&lang, "当前", "Current");
    let tray = state.node_menu.lock().unwrap_or_else(|e| e.into_inner());
    tray.snapshot(&node_label, &current_label)
}

pub(crate) fn tray_menu_entries(
    app: &AppHandle,
    #[cfg(target_os = "macos")] data: &crate::tray::state::TrayMenuData,
) -> Vec<crate::tray::menu::Entry> {
    use crate::tray::menu::Entry;
    let state = app.state::<AppState>();
    let lang = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .ui
        .lang
        .clone();

    let sp = state.sysproxy.status();
    #[cfg(target_os = "macos")]
    let tun_enabled = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .engine
        .tun_enable;
    let proxy_label = if !platform::capabilities().system_proxy {
        tr(
            &lang,
            "系统代理：当前平台暂不支持",
            "System proxy: unavailable on this platform",
        )
    } else if sp.enabled {
        format!(
            "{} :{}",
            tr(&lang, "系统代理：已开启", "System Proxy: On"),
            sp.port
        )
    } else {
        tr(&lang, "系统代理：已关闭", "System Proxy: Off")
    };
    let proxy = Entry::action(
        "toggle-proxy",
        proxy_label,
        platform::capabilities().system_proxy,
    );
    #[cfg(target_os = "macos")]
    let tun_label = if tun_enabled {
        if state.core().status() == CoreStatus::Running {
            tr(&lang, "TUN 模式：运行中", "TUN mode: Running")
        } else {
            tr(
                &lang,
                "TUN 已开启 · 内核未运行",
                "TUN enabled · core stopped",
            )
        }
    } else {
        tr(&lang, "TUN 模式：已关闭", "TUN mode: Off")
    };
    #[cfg(target_os = "macos")]
    let tun_item = Entry::action("toggle-tun", tun_label, true);

    let mode = state.mode.lock().unwrap_or_else(|e| e.into_inner()).clone();
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
    let mode_menu = Entry::Submenu(
        mode_label,
        vec![
            Entry::check("mode-direct", tr(&lang, "直连", "Direct"), mode == "direct"),
            Entry::check("mode-rule", tr(&lang, "规则", "Rule"), mode == "rule"),
            Entry::check("mode-global", tr(&lang, "全局", "Global"), mode == "global"),
        ],
    );

    let (profiles, active) = {
        let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        (
            store.data().profiles.clone(),
            store.data().active_profile.clone(),
        )
    };
    let profile_menu = Entry::Submenu(
        tr(&lang, "配置", "Profiles"),
        profiles
            .iter()
            .map(|profile| {
                Entry::check(
                    format!("profile-{}", profile.id),
                    profile.name.clone(),
                    active.as_deref() == Some(profile.id.as_str()),
                )
            })
            .collect(),
    );

    let mut entries = vec![proxy];
    #[cfg(target_os = "macos")]
    entries.push(tun_item);
    entries.extend([Entry::Separator, mode_menu]);
    #[cfg(target_os = "macos")]
    {
        if !data.rows.is_empty() {
            let label = if data.testing {
                tr(&lang, "延迟测速中…", "Testing latency…")
            } else {
                tr(&lang, "延迟测速", "Test latency")
            };
            let mut nodes = vec![
                Entry::action("tray-delay-test", label, !data.testing),
                Entry::Separator,
            ];
            nodes.extend(data.rows.iter().map(|row| {
                Entry::check(
                    format!("node-{}", row.name),
                    row.label.clone(),
                    row.selected,
                )
            }));
            entries.push(Entry::Submenu(
                crate::tray::speed::format_flag_spacing(&data.title),
                nodes,
            ));
        }
    }
    entries.extend([profile_menu, Entry::Separator]);
    #[cfg(target_os = "macos")]
    if let Some(label) = update_check::tray_label(app) {
        entries.push(Entry::action("tray-update", label, true));
    }
    entries.extend([
        Entry::action("open", tr(&lang, "打开主窗口", "Open Window"), true),
        Entry::Separator,
        Entry::action(
            "quit",
            tr(&lang, "退出（还原系统代理）", "Quit (restore proxy)"),
            true,
        ),
    ]);
    entries
}

pub(crate) fn refresh_tray(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    crate::tray::speed::refresh_menu(app);

    #[cfg(not(target_os = "macos"))]
    {
        // Release the tray lock before menu operations dispatch to the main thread.
        let tray = app
            .state::<AppState>()
            .tray
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let Some(tray) = tray else { return };
        match crate::tray::menu::build(app, &tray_menu_entries(app)) {
            Ok(menu) => {
                let _ = tray.set_menu(Some(menu));
            }
            Err(e) => eprintln!("rebuild tray menu: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_recovery_failure_notice_offers_retry_with_error_details() {
        let notice = sysproxy_recovery_failure_notice("authorization was cancelled".into());

        assert_eq!(notice.kind, notifications::Kind::Error);
        assert_eq!(notice.code, "sysproxy.restoreFailed");
        assert_eq!(notice.source, SYS_PROXY_RECOVERY_SOURCE);
        assert_eq!(notice.details, "authorization was cancelled");
        assert!(notice
            .actions
            .iter()
            .any(|action| { matches!(action, notifications::Action::RetrySystemProxy) }));
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "windows")))]
mod baseline_tests {
    use super::*;

    /// #481 判定矩阵：国内直连失败 = 本机断网（充分信号，国外结果无关）。
    #[test]
    fn baseline_verdict_reports_outage_when_cn_direct_fails() {
        let (code, kind) = baseline_verdict(false, false);
        assert_eq!(code, "tray.networkDown");
        assert!(matches!(kind, notifications::Kind::Warning));
        let (code, kind) = baseline_verdict(false, true);
        assert_eq!(code, "tray.networkDown");
        assert!(matches!(kind, notifications::Kind::Warning));
    }

    /// 国内可达 + 国外失败：墙是常态，测速又全超时 → 节点/机场故障。
    #[test]
    fn baseline_verdict_reports_node_outage_when_only_global_fails() {
        let (code, kind) = baseline_verdict(true, false);
        assert_eq!(code, "tray.nodesDown");
        assert!(matches!(kind, notifications::Kind::Warning));
    }

    /// 国内外都可达但测速全超时：提示测速异常（Info，低优先不惊扰）。
    #[test]
    fn baseline_verdict_reports_anomaly_when_both_probes_pass() {
        let (code, kind) = baseline_verdict(true, true);
        assert_eq!(code, "tray.speedTestAnomaly");
        assert!(matches!(kind, notifications::Kind::Info));
    }
}
