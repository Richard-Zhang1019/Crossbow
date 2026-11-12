//! Linux TUN transitions share CoreManager; the service owns network resources.
use crate::{AppState, CoreStatus};

pub fn toggle_tun(state: &AppState, enable: bool) -> Result<(), String> {
    let _operation = state.core_op.lock().unwrap_or_else(|e| e.into_inner());
    state.exit.ensure_open()?;
    let _recovery =
        crate::engine::config_transition::RecoveryOwner::enter(&state.config_transition);
    let was_running = matches!(
        state.core().status(),
        CoreStatus::Running | CoreStatus::Starting
    );
    let actual = match crate::network::linux_service::status() {
        Ok(session) => Some(session.is_some_and(|s| s.active)),
        Err(error) if enable => return Err(error),
        Err(_) => None, // Closing must still retry a disconnected service's cleanup.
    };
    if actual == Some(enable) && enable == state.store.lock().unwrap().data().engine.tun_enable {
        return Ok(());
    }
    if enable {
        let (engine, wireguard_coexist) = {
            let store = state.store.lock().unwrap();
            let engine = &store.data().engine;
            (engine.engine, engine.linux_wireguard_coexist)
        };
        if engine != crossbow_core::Engine::Mihomo {
            return Err("Linux TUN 当前仅支持 mihomo，请先切换内核".into());
        }
        crate::render_current(state, true)?;
        let own = state.core().pid().into_iter().collect::<Vec<_>>();
        crossbow_service::linux_network::ensure_available_with_wireguard(
            None,
            &own,
            wireguard_coexist,
        )?;
        // Authorization and conflict checks precede stopping the user's proxy.
        crate::network::linux_service::authorize(wireguard_coexist)?;
    }
    if !state.core().stop() {
        return Err("内核或网络资源未完全释放，TUN 状态未改变；请重试停止".into());
    }
    state.store.lock().unwrap().data_mut().engine.tun_enable = enable;
    if enable || was_running {
        let started = crate::start_core_locked(state)
            .and_then(|()| crate::wait_core_running(state))
            .and_then(|()| {
                if enable && !crate::network::linux_service::status()?.is_some_and(|s| s.active) {
                    return Err("内核已启动，但服务未确认网络接管".into());
                }
                Ok(())
            });
        if let Err(error) = started {
            if !state.core().stop() {
                return Err(format!(
                    "TUN 切换失败：{error}；资源清理未确认，请重试关闭 TUN"
                ));
            }
            state.store.lock().unwrap().data_mut().engine.tun_enable = false;
            let recovery = if enable && was_running {
                crate::start_core_locked(state).and_then(|()| crate::wait_core_running(state))
            } else {
                Ok(())
            };
            let saved = state
                .store
                .lock()
                .unwrap()
                .save()
                .map_err(|e| e.to_string());
            return Err(format!(
                "TUN 切换失败：{error}{}{}",
                recovery
                    .err()
                    .map(|e| format!("；显式代理恢复失败：{e}"))
                    .unwrap_or_default(),
                saved
                    .err()
                    .map(|e| format!("；状态保存失败：{e}"))
                    .unwrap_or_default()
            ));
        }
    }
    // Persist only after the service confirmed the requested network state.
    state
        .store
        .lock()
        .unwrap()
        .save()
        .map_err(|e| e.to_string())
}

pub fn set_wireguard_coexist(state: &AppState, enabled: bool) -> Result<(), String> {
    let _operation = state
        .core_op
        .try_lock()
        .map_err(|_| "请等待当前内核操作完成后重试")?;
    if crate::network::linux_service::status()?.is_some() {
        return Err("请关闭 TUN 并确认网络已恢复后再更改 WireGuard 共存设置".into());
    }
    let mut store = state.store.lock().unwrap();
    let previous = store.data().engine.linux_wireguard_coexist;
    let tun_enabled = store.data().engine.tun_enable;
    if previous != enabled || tun_enabled {
        store.data_mut().engine.linux_wireguard_coexist = enabled;
        store.data_mut().engine.tun_enable = false;
        if let Err(error) = store.save() {
            store.data_mut().engine.linux_wireguard_coexist = previous;
            store.data_mut().engine.tun_enable = tun_enabled;
            return Err(error.to_string());
        }
    }
    Ok(())
}
