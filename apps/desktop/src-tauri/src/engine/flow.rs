// 内核启动/渲染/提交管线 helper（#423 Stage B 从 lib.rs 提取）。
use crate::*;

const CONFIG_OUTBOUND_PROBE_TARGETS: [&str; 2] = [
    "https://www.gstatic.com/generate_204",
    "https://cp.cloudflare.com/generate_204",
];
/// 渲染当前配置（记录覆写错误到 store，供 UI 展示）。
pub(crate) fn render_current(
    state: &AppState,
    use_overrides: bool,
) -> Result<crossbow_core::Rendered, String> {
    let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
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
pub(crate) fn start_core(state: &AppState) -> Result<(), String> {
    state.exit.ensure_open()?;
    let _op = state.core_op.lock().unwrap_or_else(|e| e.into_inner());
    state.exit.ensure_open()?;
    // A diagnostic result can predate a successful rollback or another start.
    // Starting means ensure-running, never stop/reject an already healthy core.
    match state.core().status() {
        CoreStatus::Running => return Ok(()),
        CoreStatus::Starting => return wait_core_running(state),
        _ => {}
    }
    start_core_locked(state)
}

/// A successful spawn only means the process exists. Do not expose TUN or
/// system proxy as enabled until the controller confirms the core is ready.
pub(crate) fn wait_core_running(state: &AppState) -> Result<(), String> {
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
        let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store.data().engine.engine == crossbow_core::Engine::SingBox
    };
    let need = if is_singbox { "sing-box" } else { "mihomo" };
    #[cfg(target_os = "linux")]
    let _ = ensure_engine_binary(state, need)?;
    #[cfg(not(target_os = "linux"))]
    let mut binary = ensure_engine_binary(state, need)?;
    let rendered = render_current(state, !safe)?;
    // CoreManager::start synchronously notifies the status callback, which also
    // reads the store. Release this guard before either TUN or regular startup.
    let rt = {
        let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        let tun_enable = store.data().engine.tun_enable;
        // v6 双栈注入与原生 v6 可达性联动（#136）：v4-only 网络上 DIRECT
        // 的 v6 目的地发不出去，只会在日志里刷 no route to host。
        #[cfg(target_os = "macos")]
        let native_ipv6 = !tun_enable || crate::network::tun::has_native_ipv6();
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
            binary = crate::network::tun::ensure_root_binary(state, &binary)?;
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
            return state
                .core()
                .start_with_spawner(&rendered.config, &rt, Box::new(spawner));
        }
        #[cfg(target_os = "linux")]
        {
            if is_singbox {
                return Err("Linux TUN 当前仅支持 mihomo".into());
            }
            let wireguard_coexist = state
                .store
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .data()
                .engine
                .linux_wireguard_coexist;
            crossbow_service::linux_network::ensure_available_with_wireguard(
                None,
                &[],
                wireguard_coexist,
            )?;
            let spawner = crate::network::linux_service::make_core_spawner(wireguard_coexist)?;
            return state
                .core()
                .start_with_spawner(&rendered.config, &rt, Box::new(spawner));
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

pub(crate) fn missing_core_error(need: &str) -> String {
    format!("内核「{need}」未安装 — 到设置页下载对应内核后再启动")
}

/// 管理器持有的二进制与目标引擎不一致时（引擎切换、冷启动回退），就地换核。
/// 返回换核后实际使用的二进制路径。
pub(crate) fn ensure_engine_binary(state: &AppState, need: &str) -> Result<PathBuf, String> {
    let cur = state.core().binary_path();
    state.core().log_decision(&format!(
        "ensure: need={need} cur={} cur_exists={}",
        cur.display(),
        cur.exists()
    ));
    if cur.exists() && crate::engine::download::is_core_bin(&cur, need) {
        return Ok(cur);
    }
    let Some(p) = crate::engine::download::resolve_core_for(&state.data_dir, Some(need)) else {
        return Err(missing_core_error(need));
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
pub(crate) fn ensure_unprivileged_core_binary(
    state: &AppState,
    need: &str,
    current: PathBuf,
) -> Result<PathBuf, String> {
    let tun_dir = state.data_dir.join("tun-binaries");
    if !current.starts_with(&tun_dir) {
        return Ok(current);
    }
    let Some(binary) = crate::engine::download::resolve_core_for(&state.data_dir, Some(need))
    else {
        return Err(missing_core_error(need));
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
/// 在内核确认就绪且配置切换未破坏已验证的出口，或内核本来没在跑时，才落盘。
///
/// TUN 开关不走这里。它要换特权进程，仍由各平台 `toggle_tun` 处理。
pub(crate) fn commit_config_change(
    state: &AppState,
    mutate: impl FnOnce(&mut crossbow_core::StoreSchema) -> Result<(), String>,
) -> Result<(), String> {
    commit_config_change_inner(state, mutate, false)
}

pub(crate) fn commit_config_change_inner(
    state: &AppState,
    mutate: impl FnOnce(&mut crossbow_core::StoreSchema) -> Result<(), String>,
    exit_safe_mode: bool,
) -> Result<(), String> {
    let _op = state.core_op.lock().unwrap_or_else(|e| e.into_inner());
    state.exit.ensure_open()?;
    let _recovery =
        crate::engine::config_transition::RecoveryOwner::enter(&state.config_transition);
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
    let live = crate::engine::config_transition::LiveCore {
        running,
        mihomo: before.engine.engine == crossbow_core::Engine::Mihomo
            && status == CoreStatus::Running,
        controller: state.core().controller().is_some(),
        cached_config: old_config.is_some(),
        service_managed: cfg!(any(windows, target_os = "linux")) && before.engine.tun_enable,
    };
    let plan = if crate::engine::config_transition::needs_new_process(
        before.engine.engine,
        after.engine.engine,
        before.engine.mixed_port,
        after.engine.mixed_port,
        before.engine.allow_lan,
        after.engine.allow_lan,
        before.engine.tun_enable,
        after.engine.tun_enable,
    ) {
        crate::engine::config_transition::plan_identity(live)
    } else {
        crate::engine::config_transition::plan_content(live)
    };
    state
        .core()
        .log_decision(&format!("config change: {plan:?}"));
    let old_content = before
        .active_profile
        .as_deref()
        .and_then(|id| before.profile(id));
    let new_content = after
        .active_profile
        .as_deref()
        .and_then(|id| after.profile(id));
    let verify_outbound = live.mihomo
        && after.engine.engine == crossbow_core::Engine::Mihomo
        && (before.active_profile != after.active_profile
            || old_content.map(|p| &p.content) != new_content.map(|p| &p.content));
    let mut transaction = ConfigChange {
        state,
        before,
        previous_safe,
        proxy,
        old_config,
        plan,
        verify_outbound,
        touched_core: false,
        restarted: false,
        stopped: false,
    };
    let result = {
        // Serialize manual node selections with the pre/post measurements.
        let _selection =
            verify_outbound.then(|| state.proxy_op.lock().unwrap_or_else(|e| e.into_inner()));
        crate::engine::config_transition::execute(&mut transaction)
    };
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

pub(crate) struct ConfigChange<'a> {
    state: &'a AppState,
    before: crossbow_core::StoreSchema,
    previous_safe: bool,
    proxy: SysProxyStatus,
    old_config: Option<String>,
    plan: crate::engine::config_transition::Plan,
    verify_outbound: bool,
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

impl crate::engine::config_transition::Transaction for ConfigChange<'_> {
    fn probe_outbound(&mut self) -> Option<bool> {
        if !self.verify_outbound {
            return None;
        }
        controller_client(self.state)
            .ok()?
            .probe_config_outbound(CONFIG_OUTBOUND_PROBE_TARGETS)
    }

    fn apply(&mut self) -> Result<(), String> {
        if self.plan == crate::engine::config_transition::Plan::PersistOnly {
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
        if self.plan == crate::engine::config_transition::Plan::HotReload {
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

pub(crate) fn reload_final_config(state: &AppState, config: &str) -> Result<(), String> {
    let (port, secret) = state
        .core()
        .controller()
        .ok_or("控制器不可用，无法热重载")?;
    let core = state.core();
    core.write_config_file(config)?;
    crate::engine::api::Controller { port, secret }.reload_config(&core.config_path())?;
    core.replace_cached_config(config.to_string());
    Ok(())
}

pub(crate) fn restore_config_proxy(
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

pub(crate) fn runtime_for(
    state: &AppState,
    controller_port: u16,
    controller_secret: String,
) -> RuntimeConfig {
    let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
    let tun_enable = store.data().engine.tun_enable;
    #[cfg(target_os = "macos")]
    let native_ipv6 = !tun_enable || crate::network::tun::has_native_ipv6();
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

pub(crate) fn persist_schema(state: &AppState) -> Result<(), String> {
    let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
    store.save().map_err(|e| e.to_string())
}

/// Restore in-memory state before attempting I/O; a failed save must not
/// leave the rejected candidate visible to the next operation.
pub(crate) fn restore_store(
    store: &Mutex<Store>,
    before: &crossbow_core::StoreSchema,
) -> Result<(), String> {
    let mut store = store.lock().unwrap_or_else(|e| e.into_inner());
    *store.data_mut() = before.clone();
    store.save().map_err(|e| e.to_string())
}

pub(crate) fn recover_config_network(state: &AppState) -> Result<(), String> {
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
    #[cfg(target_os = "macos")]
    if tun {
        state.core().log_decision(&format!(
            "tun cleanup: system_proxy_restored={} {}",
            proxy.is_ok() && tun_proxy.is_ok(),
            crate::engine::manager::tun_route_diagnostics(),
        ));
    }
    match (proxy, tun_proxy) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(a), Err(b)) => Err(format!("系统代理恢复失败：{a}；TUN 代理恢复失败：{b}")),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

#[derive(serde::Serialize)]
pub(crate) struct TunStatus {
    pub(crate) enabled: bool,
    /// 内核二进制已 setuid root（可无感开 TUN）
    pub(crate) ready: bool,
    pub(crate) core_running: bool,
}

// ---------- 内核与系统代理命令 ----------

// ---------- WS 数据桥命令（页面挂载订阅、卸载退订） ----------

pub(crate) fn require_controller(state: &AppState) -> Result<(u16, String), String> {
    state
        .core()
        .controller()
        .ok_or_else(|| "内核未运行".to_string())
}

// ---------- 诊断 ----------

/// 从最终配置提取兜底出口目标：mihomo YAML 取 MATCH 行的 target；
/// sing-box JSON 取 route.final。/rules 无 MATCH 规则时（sing-box）用它。
pub(crate) fn extract_final_outbound(cfg: &str) -> Option<String> {
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

// ---------- 内核安装命令 ----------

pub(crate) fn core_install_inner(
    app: AppHandle,
    engine: Option<String>,
) -> Result<CoreBinaryInfo, String> {
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let kind = match engine.as_deref() {
        Some("singbox") => crate::engine::download::CoreKind::SingBox,
        _ => crate::engine::download::CoreKind::Mihomo,
    };
    let h = app.clone();
    let info =
        crate::engine::download::install_core(&data_dir, kind, &move |p: InstallProgress| {
            let mut notice = notifications::Notice::new(
                notifications::Kind::Progress,
                "core.install",
                "core-download",
            );
            notice.description = p.stage.clone();
            notifications::publish(&h, notice);
            let _ = h.emit("core://install-progress", p);
        })?;
    // 安装后重新装载 CoreManager 的二进制路径：直接替换状态里的管理器。
    let state = app.state::<AppState>();
    // 与启动链互斥：换核期间不允许并发的 start_core 拿到半新半旧的槽位
    let _op = state.core_op.lock().unwrap_or_else(|e| e.into_inner());
    state.exit.ensure_open()?;
    let new_core = CoreManager::new(info.clone(), data_dir.join("runtime"));
    new_core.set_callback(core_status_callback(app.clone()));
    if !state.core().stop() {
        return Err("旧内核未退出，无法载入已下载的内核".into());
    }
    *state.core_slot.lock().unwrap_or_else(|e| e.into_inner()) = new_core;
    Ok(CoreBinaryInfo {
        path: info.display().to_string(),
        source: "data".into(),
        version: core_version_of(&info).unwrap_or_default(),
    })
}

/// 内核版本（执行 `<bin> -v` 首行）；sing-box 只认 `version` 子命令，依次尝试。
pub(crate) fn core_version_of(bin: &PathBuf) -> Option<String> {
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

pub(crate) fn controller_client(
    state: &AppState,
) -> Result<crate::engine::api::Controller, String> {
    let (port, secret) = state
        .core()
        .controller()
        .ok_or_else(|| "内核未运行".to_string())?;
    Ok(crate::engine::api::Controller { port, secret })
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
                        .filter_map(|g| g.get("name").and_then(|n| n.as_str()).map(String::from))
                        .collect()
                })
        })
        .unwrap_or_default()
}

pub(crate) fn select_proxy_inner(state: &AppState, group: &str, name: &str) -> Result<(), String> {
    let _op = state.proxy_op.lock().unwrap_or_else(|e| e.into_inner());
    controller_client(state)?.select_proxy(group, name)
}

// ---------- 设置命令 ----------

#[derive(serde::Serialize)]
pub(crate) struct EngineView {
    pub(crate) engine: String,
    pub(crate) mixed_port: u16,
    pub(crate) allow_lan: bool,
    pub(crate) flag_emoji: bool,
    pub(crate) linux_wireguard_coexist: bool,
    /// 当前引擎对应内核是否已安装。
    pub(crate) core_installed: bool,
}

// Network probes run off the GUI thread with bounded timeouts.

// 必须异步 + spawn_blocking：detect 内部跑 ps/netstat 子进程（秒级），
// 同步命令会在主线程执行直接卡死整个 UI（实测 hang 1.07s 报告）。

// ---------- 深链接 ----------
