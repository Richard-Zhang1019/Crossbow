//! SSID 网络场景引擎（#566）：按 Wi-Fi 名称自动切换接管动作。
//!
//! 触发点：network_watch 网络变化、应用启动、设置页变更。判定是纯函数
//! （单测覆盖）；执行复用既有开关链路（`toggle_sysproxy_state` /
//! `toggle_tun`，自带 journal/操作锁/通知语义）。进入场景前的接管状态
//! 保存在进程内存，离开时恢复；重启后按启动时网络重新评估（不追溯）。

use std::sync::Mutex;

use tauri::{AppHandle, Manager};

use super::ssid;
use crate::AppState;

pub(crate) const ACTION_DIRECT: &str = "direct";
pub(crate) const ACTION_PROXY: &str = "proxy";

/// 进入场景前的接管状态快照；离开场景时恢复。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entered {
    ssid: String,
    action: String,
    prev_sysproxy: bool,
    prev_tun: bool,
}

static ENTERED: Mutex<Option<Entered>> = Mutex::new(None);

/// 已进入的场景（设置页状态显示用）：（SSID, 动作）。
pub(crate) fn active_scene() -> Option<(String, String)> {
    let entered = ENTERED.lock().unwrap_or_else(|e| e.into_inner());
    let entered = entered.as_ref()?;
    Some((entered.ssid.clone(), entered.action.clone()))
}

/// 状态转移判定（纯函数）：
/// - 未进入 + 无目标 → 维持；未进入 + 有目标 → 进入；
/// - 已进入 + 无目标 → 离开恢复；已进入 + 同网络目标 → 维持；
/// - 已进入 + 另一网络目标 → 切换（先恢复前场景，再进入新场景）。
fn transition(entered: Option<&Entered>, target: Option<(&str, &str)>) -> Transition {
    match (entered, target) {
        (None, None) => Transition::Keep,
        (None, Some((_, action))) => Transition::Enter(action.to_string()),
        (Some(_), None) => Transition::Restore,
        (Some(current), Some((ssid, action))) => {
            if current.ssid == ssid {
                Transition::Keep
            } else {
                Transition::Switch(action.to_string())
            }
        }
    }
}

enum Transition {
    Keep,
    Enter(String),
    Restore,
    Switch(String),
}

/// 评估并应用（幂等）。动作在线程内同步执行：调用方（network_watch 事件
/// 循环）已 debounce，场景切换本身不高频；拿不到锁的操作会以错误通知收敛。
pub(crate) fn evaluate_and_apply(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let rules = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .scenes
        .clone();
    let ssid = ssid::current_ssid();
    let target = rules
        .target_action(ssid.as_deref())
        .map(|action| (ssid.clone().unwrap_or_default(), action.to_owned()));

    let transition = {
        let entered = ENTERED.lock().unwrap_or_else(|e| e.into_inner());
        transition(
            entered.as_ref(),
            target.as_ref().map(|(s, a)| (s.as_str(), a.as_str())),
        )
    };
    match transition {
        Transition::Keep => {}
        Transition::Enter(action) => {
            if let Some((ssid, _)) = target {
                enter(app, ssid, action);
            }
        }
        Transition::Restore => leave_and_restore(app),
        Transition::Switch(action) => {
            leave_and_restore(app);
            if let Some((ssid, _)) = target {
                enter(app, ssid, action);
            }
        }
    }
}

/// 进入场景：记录进入前状态并应用动作。
fn enter(app: &AppHandle, ssid: String, action: String) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let prev_sysproxy = state.sysproxy.status().enabled;
    let prev_tun = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .engine
        .tun_enable;
    let action_label = if action == ACTION_DIRECT {
        ACTION_DIRECT
    } else {
        ACTION_PROXY
    };
    let applied = apply_action(&state, action_label);
    match applied {
        Ok(()) => {
            *ENTERED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Entered {
                ssid: ssid.clone(),
                action: action_label.to_string(),
                prev_sysproxy,
                prev_tun,
            });
            notify_scene(app, &format!("scene.applied.{action_label}"), &ssid, None);
        }
        Err(error) => {
            // 动作失败：不登记进入状态（下次网络事件会重试），并如实通知。
            notify_scene(app, "scene.failed", &ssid, Some(error));
        }
    }
}

/// 离开场景：恢复进入前的接管状态。
fn leave_and_restore(app: &AppHandle) {
    let Some(entered) = ENTERED.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    if let Err(error) = restore(&state, &entered) {
        // 恢复失败：不重登记（避免循环），journal 与下次启动自愈兜底。
        crate::notifications::result(app, "scene-restore", "scene.restore", Err(error));
        return;
    }
    let mut restored = crate::notifications::Notice::new(
        crate::notifications::Kind::Info,
        "scene.restored",
        "scene",
    );
    restored.title_key = "scene.restored".to_string();
    crate::notifications::publish(app, restored);
}

fn restore(state: &AppState, entered: &Entered) -> Result<(), String> {
    set_tun(state, entered.prev_tun)?;
    set_sysproxy(state, entered.prev_sysproxy)
}

/// 按目标动作接管（幂等：已是目标状态则跳过）。
fn apply_action(state: &AppState, action: &str) -> Result<(), String> {
    match action {
        ACTION_DIRECT => {
            set_tun(state, false)?;
            set_sysproxy(state, false)
        }
        _ => {
            set_sysproxy(state, true)?;
            set_tun(state, true)
        }
    }
}

fn set_sysproxy(state: &AppState, enable: bool) -> Result<(), String> {
    if state.sysproxy.status().enabled == enable {
        return Ok(());
    }
    crate::toggle_sysproxy_state(state)
}

fn set_tun(state: &AppState, enable: bool) -> Result<(), String> {
    let current = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .engine
        .tun_enable;
    if current == enable {
        return Ok(());
    }
    // 平台分派与 set_tun 命令一致（Linux 服务托管 / Windows SYSTEM 服务）。
    #[cfg(target_os = "macos")]
    {
        crate::network::tun::toggle_tun(state, enable)
    }
    #[cfg(target_os = "linux")]
    {
        crate::network::tun_linux::toggle_tun(state, enable)
    }
    #[cfg(windows)]
    {
        crate::network::tun_windows::toggle_tun(state, enable)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        Err("当前平台不支持 TUN".into())
    }
}

fn notify_scene(app: &AppHandle, code: &str, ssid: &str, error: Option<String>) {
    // 标题走 titleKey 双语查表（键含动作，如 scene.applied.direct）；
    // description 放语言无关的网络名。失败走 Error 通知 + 脱敏详情。
    let mut notice =
        crate::notifications::Notice::new(crate::notifications::Kind::Info, code, "scene");
    notice.title_key = code.to_string();
    notice.description = ssid.to_string();
    if let Some(error) = error {
        notice.kind = crate::notifications::Kind::Error;
        notice.details = error;
    }
    crate::notifications::publish(app, notice);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entered(ssid: &str) -> Entered {
        Entered {
            ssid: ssid.to_string(),
            action: ACTION_DIRECT.to_string(),
            prev_sysproxy: true,
            prev_tun: false,
        }
    }

    #[test]
    fn transition_covers_all_five_states() {
        let home = entered("Home");
        // 未进入：无目标维持 / 有目标进入
        assert!(matches!(transition(None, None), Transition::Keep));
        assert!(matches!(
            transition(None, Some(("Home", "direct"))),
            Transition::Enter(_)
        ));
        // 已进入：无目标恢复 / 同网络维持 / 异网络切换
        assert!(matches!(transition(Some(&home), None), Transition::Restore));
        assert!(matches!(
            transition(Some(&home), Some(("Home", "direct"))),
            Transition::Keep
        ));
        assert!(matches!(
            transition(Some(&home), Some(("Office", "proxy"))),
            Transition::Switch(_)
        ));
    }
}
