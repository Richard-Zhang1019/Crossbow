//! Windows TUN 切换（#81）。
//!
//! 内核创建 wintun 适配器需要管理员权限，GUI 进程无从提权——TUN 内核由
//! 特权服务（crossbow-service，NSIS 注册开机自启）以 SYSTEM 拉起；端口/
//! 密钥生成、配置注入、就绪探测与状态簿记复用
//! `CoreManager::start_with_spawner`。与 macOS 版 toggle_tun 相比没有
//! setuid/授权副本概念：服务装好即授权持久化。

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use crate::AppState;

pub fn toggle_tun(state: &AppState, enable: bool) -> Result<(), String> {
    // 与 macOS 相同：整段切换与启动/健康监测串行化，存储标志不得跑在
    // 失败的停止前面。
    let _operation = state.core_op.lock().unwrap_or_else(|e| e.into_inner());
    state.exit.ensure_open()?;
    toggle_tun_locked(state, enable)
}

/// 连通性失败时关闭。持锁后再读，避免关掉用户刚刚打开的 TUN。
pub fn disable_if_enabled(state: &AppState) -> Result<(), String> {
    let _operation = state.core_op.lock().unwrap_or_else(|e| e.into_inner());
    state.exit.ensure_open()?;
    let enabled = {
        let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store.data().engine.tun_enable
    };
    if !enabled {
        return Ok(());
    }
    toggle_tun_locked(state, false)
}

fn toggle_tun_locked(state: &AppState, enable: bool) -> Result<(), String> {
    let previous_tun = {
        let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store.data().engine.tun_enable
    };
    if previous_tun == enable {
        return Ok(());
    }
    let was_running = matches!(
        state.core().status(),
        crate::CoreStatus::Running | crate::CoreStatus::Starting
    );
    if enable {
        if !crate::service_client::installed() {
            crate::service_client::install_elevated()?;
        }
        crate::service_client::running()?;
    }
    let previous_proxy = state.sysproxy.status();
    let suspended_proxy = if enable && !previous_proxy.enabled {
        Some(state.sysproxy.suspend_for_tun()?)
    } else {
        None
    };

    // 与 macOS 相同：先停现有内核再翻转。CoreManager 的启动预检要求
    // Stopped/Crashed，直接 start 会报「core is Running, stop it first」
    // （v0.11.0 实测，#159 用户反馈）。服务托管的 SYSTEM 内核由 stop()
    // 内部路由给特权服务停止，用户态内核走常规路径。
    if was_running && !state.core().stop() {
        if let Some(suspended) = suspended_proxy {
            state.sysproxy.restore_after_failed_tun(suspended)?;
        }
        return Err("旧内核未退出，TUN 状态未改变；请重试切换".into());
    }

    {
        let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store.data_mut().engine.tun_enable = enable;
        if let Err(error) = store.save() {
            store.data_mut().engine.tun_enable = previous_tun;
            drop(store);
            if let Some(suspended) = suspended_proxy {
                state.sysproxy.restore_after_failed_tun(suspended)?;
            }
            return Err(error.to_string());
        }
    }

    // 开 TUN 必须有内核；关 TUN 时若原本有内核，重启为非 TUN 模式。
    // 内核的拉起/停止在 start_core_locked 内部分流到服务（SYSTEM）。
    let should_restart = was_running || enable || previous_tun;
    if should_restart {
        let start_result = crate::start_core_locked(state)
            .and_then(|()| crate::wait_core_running(state))
            .and_then(|()| {
                if enable {
                    wait_tun_active(state)
                } else {
                    Ok(())
                }
            });
        if let Err(start_error) = start_result {
            {
                let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
                store.data_mut().engine.tun_enable = previous_tun;
                let _ = store.save();
            }
            if !enable {
                // 旧的 SYSTEM 内核已由服务停掉；回滚时恢复非 TUN 内核
                let _ = crate::service_client::stop_core();
            }
            let recovery_error = (was_running || previous_proxy.enabled)
                .then(|| crate::start_core_locked(state).err())
                .flatten();
            let proxy_rollback_error = if previous_proxy.enabled && recovery_error.is_some() {
                state.sysproxy.disable().err()
            } else {
                suspended_proxy
                    .and_then(|suspended| state.sysproxy.restore_after_failed_tun(suspended).err())
            };
            return Err(format!(
                "TUN 切换失败已回滚：{start_error}{}",
                proxy_rollback_error
                    .map(|e| format!("；代理恢复失败：{e}"))
                    .unwrap_or_default()
            ));
        }
    }
    if !enable {
        let _ = state.sysproxy.restore_after_tun();
    }
    Ok(())
}

/// 就绪判定：控制器报告 tun 已启用且适配器在位（wintun 建立通常 <1s，
/// 服务拉起内核 + 探测窗口兜底 15s）。
fn wait_tun_active(state: &AppState) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some((port, secret)) = state.core().controller() {
            let client = crate::mihomo_api::Controller { port, secret };
            if let Ok(Some(_device)) = client.active_tun_device() {
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            return Err("TUN 适配器未在预期时间内建立（查看内核日志获取详情）".into());
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// 供 coexist/诊断使用：本应用 TUN 适配器名（runtime 注入的 device）
pub const TUN_DEVICE: &str = "Crossbow";

/// 兼容 macOS toggle 的路由预检签名；Windows 无 netstat 分流路由概念，
/// 冲突检测交给 coexist 的适配器识别（#82），此处恒通过。
pub fn ensure_route_available() -> Result<BTreeSet<(String, String)>, String> {
    Ok(BTreeSet::new())
}
