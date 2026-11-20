//! SSID scene execution with a durable pre-scene baseline and serialized evaluation.
use super::ssid;
use crate::AppState;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

pub(crate) const ACTION_DIRECT: &str = "direct";
pub(crate) const ACTION_PROXY: &str = "proxy";
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Takeover {
    sysproxy: bool,
    tun: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Entered {
    ssid: String,
    action: String,
    baseline: Takeover,
    applied: bool,
}
static ENTERED: Mutex<Option<Entered>> = Mutex::new(None);
static EVALUATION: Mutex<()> = Mutex::new(());

fn current_takeover(state: &AppState) -> Takeover {
    Takeover {
        sysproxy: state.sysproxy.status().enabled,
        tun: state
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .data()
            .engine
            .tun_enable,
    }
}

pub(crate) fn active_scene(state: &AppState) -> Option<(String, String)> {
    let current = current_takeover(state);
    ENTERED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .filter(|e| e.applied && target_state(&e.action, e.baseline) == current)
        .map(|e| (e.ssid.clone(), e.action.clone()))
}

enum Outcome {
    Keep,
    Applied(String, String),
    Restored,
    Failed(String),
}
fn target_state(action: &str, baseline: Takeover) -> Takeover {
    if action == ACTION_DIRECT {
        Takeover {
            sysproxy: false,
            tun: false,
        }
    } else {
        Takeover {
            sysproxy: baseline.sysproxy || !baseline.tun,
            tun: baseline.tun,
        }
    }
}

// Persistence happens before any system mutation. Failures retain the original
// baseline, including partially applied transitions, so later retries can recover.
fn reconcile(
    entered: &mut Option<Entered>,
    target: Option<(&str, &str)>,
    current: Takeover,
    mut apply: impl FnMut(Takeover) -> Result<(), String>,
    mut persist: impl FnMut(Option<&Entered>) -> Result<(), String>,
) -> Outcome {
    if let Some((ssid, action)) = target {
        if entered
            .as_ref()
            .is_some_and(|e| e.applied && e.ssid == ssid && e.action == action)
        {
            return Outcome::Keep;
        }
        let baseline = entered.as_ref().map_or(current, |e| e.baseline);
        let mut next = Entered {
            ssid: ssid.into(),
            action: action.into(),
            baseline,
            applied: false,
        };
        if let Err(error) = persist(Some(&next)) {
            return Outcome::Failed(error);
        }
        *entered = Some(next.clone());
        if let Err(error) = apply(target_state(action, baseline)) {
            let detail = match apply(baseline) {
                Ok(()) => error,
                Err(rollback) => format!("{error}; scene rollback: {rollback}"),
            };
            return Outcome::Failed(detail);
        }
        next.applied = true;
        if let Err(error) = persist(Some(&next)) {
            return Outcome::Failed(error);
        }
        *entered = Some(next);
        Outcome::Applied(ssid.into(), action.into())
    } else {
        let Some(mut previous) = entered.clone() else {
            return Outcome::Keep;
        };
        previous.applied = false;
        if let Err(error) = persist(Some(&previous)) {
            return Outcome::Failed(error);
        }
        *entered = Some(previous.clone());
        if let Err(error) = apply(previous.baseline) {
            return Outcome::Failed(error);
        }
        if let Err(error) = persist(None) {
            return Outcome::Failed(error);
        }
        *entered = None;
        Outcome::Restored
    }
}

fn can_restore_without_ssid(
    entered: Option<&Entered>,
    rules: &crossbow_core::model::NetworkScenes,
) -> bool {
    entered.is_some_and(|e| rules.target_action(Some(&e.ssid)).is_none())
}

pub(crate) fn evaluate_and_apply(app: &AppHandle) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let _serial = EVALUATION.lock().unwrap_or_else(|e| e.into_inner());
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    if state.exit.ensure_open().is_err() {
        return;
    }
    let path = state.data_dir.join("network-scene.json");
    let mut entered = ENTERED.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if entered.is_none() {
        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Entered>(&bytes) {
                Ok(mut saved) if matches!(saved.action.as_str(), ACTION_DIRECT | ACTION_PROXY) => {
                    // A previous process's applied marker cannot certify current OS settings.
                    saved.applied = false;
                    entered = Some(saved)
                }
                _ => {
                    notify_scene(
                        app,
                        "scene.failed",
                        "",
                        Some("Invalid scene recovery journal".into()),
                    );
                    return;
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                notify_scene(app, "scene.failed", "", Some(e.to_string()));
                return;
            }
        }
    }
    let rules = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .scenes
        .clone();
    if rules.rules.is_empty() && entered.is_none() {
        let _ = app.emit("network://scene", ());
        return;
    }
    let observation = ssid::observe();
    // Failed/denied reads are not evidence of leaving Wi-Fi. An explicit rule
    // deletion is still allowed to restore its retained baseline.
    let network = match observation {
        ssid::Observation::Known(name) => Some(name),
        ssid::Observation::Disconnected => None,
        ssid::Observation::Unavailable => {
            if !can_restore_without_ssid(entered.as_ref(), &rules) {
                let _ = app.emit("network://scene", ());
                return;
            }
            None
        }
    };
    let target = rules
        .target_action(network.as_deref())
        .and_then(|action| network.as_ref().map(|s| (s.as_str(), action)));
    let current = current_takeover(&state);
    let outcome = reconcile(
        &mut entered,
        target,
        current,
        |goal| {
            if !goal.tun {
                set_tun(&state, false)?;
            }
            set_sysproxy(&state, goal.sysproxy)?;
            if goal.tun {
                set_tun(&state, true)?;
            }
            Ok(())
        },
        |record| {
            if let Some(record) = record {
                let bytes = serde_json::to_vec(record).map_err(|e| e.to_string())?;
                crate::platform::fs_perms::write_private(&path, &bytes).map_err(|e| e.to_string())
            } else {
                match std::fs::remove_file(&path) {
                    Ok(()) => Ok(()),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(e) => Err(e.to_string()),
                }
            }
        },
    );
    *ENTERED.lock().unwrap_or_else(|e| e.into_inner()) = entered;
    match outcome {
        Outcome::Keep => {
            let _ = app.emit("network://scene", ());
            return;
        }
        Outcome::Applied(name, action) => {
            notify_scene(app, &format!("scene.applied.{action}"), &name, None)
        }
        Outcome::Restored => notify_scene(app, "scene.restored", "", None),
        Outcome::Failed(error) => notify_scene(
            app,
            "scene.failed",
            network.as_deref().unwrap_or_default(),
            Some(error),
        ),
    }
    let actual = current_takeover(&state);
    if actual != current {
        let _ = app.emit("tun://status", actual.tun);
        let _ = app.emit("sysproxy://status", state.sysproxy.status());
        crate::refresh_tray(app);
    }
    let _ = app.emit("network://scene", ());
}

fn set_sysproxy(state: &AppState, enable: bool) -> Result<(), String> {
    if state.sysproxy.status().enabled == enable {
        return Ok(());
    }
    crate::change_sysproxy_state(state, Some(enable))
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
    fn baseline() -> Takeover {
        Takeover {
            sysproxy: true,
            tun: false,
        }
    }
    fn entered() -> Option<Entered> {
        Some(Entered {
            ssid: "Home".into(),
            action: "direct".into(),
            baseline: baseline(),
            applied: true,
        })
    }
    #[test]
    fn same_network_action_changes_and_switches_preserve_original_baseline() {
        let mut state = entered();
        let mut goals = Vec::new();
        assert!(matches!(
            reconcile(
                &mut state,
                Some(("Home", "proxy")),
                Takeover {
                    sysproxy: false,
                    tun: false
                },
                |g| {
                    goals.push(g);
                    Ok(())
                },
                |_| Ok(())
            ),
            Outcome::Applied(..)
        ));
        assert_eq!(goals, vec![baseline()]);
        reconcile(
            &mut state,
            Some(("Office", "direct")),
            baseline(),
            |_| Ok(()),
            |_| Ok(()),
        );
        assert_eq!(state.as_ref().unwrap().baseline, baseline());
        reconcile(
            &mut state,
            None,
            Takeover {
                sysproxy: false,
                tun: false,
            },
            |g| {
                assert_eq!(g, baseline());
                Ok(())
            },
            |_| Ok(()),
        );
        assert!(state.is_none());
    }
    #[test]
    fn proxy_never_forces_tun_on_for_a_system_proxy_user() {
        assert!(!target_state("proxy", baseline()).tun);
    }
    #[test]
    fn unchanged_and_empty_scenes_do_not_mutate_network() {
        let mut state = entered();
        assert!(matches!(
            reconcile(
                &mut state,
                Some(("Home", "direct")),
                baseline(),
                |_| panic!("must be idle"),
                |_| panic!("must be idle")
            ),
            Outcome::Keep
        ));
        assert!(matches!(
            reconcile(
                &mut None,
                None,
                baseline(),
                |_| panic!("must be idle"),
                |_| panic!("must be idle")
            ),
            Outcome::Keep
        ));
    }
    #[test]
    fn restore_failure_retains_retryable_baseline() {
        let mut state = entered();
        assert!(matches!(
            reconcile(
                &mut state,
                None,
                baseline(),
                |_| Err("restore failed".into()),
                |_| Ok(())
            ),
            Outcome::Failed(_)
        ));
        assert_eq!(state.as_ref().unwrap().baseline, baseline());
        assert!(!state.as_ref().unwrap().applied);
        reconcile(&mut state, None, baseline(), |_| Ok(()), |_| Ok(()));
        assert!(state.is_none());
    }
    #[test]
    fn partial_entry_failure_attempts_rollback_and_keeps_recovery_record() {
        let mut state = None;
        let mut calls = 0;
        reconcile(
            &mut state,
            Some(("Home", "direct")),
            baseline(),
            |goal| {
                calls += 1;
                if calls == 1 {
                    Err("partial".into())
                } else {
                    assert_eq!(goal, baseline());
                    Ok(())
                }
            },
            |_| Ok(()),
        );
        assert_eq!(calls, 2);
        assert_eq!(state.unwrap().baseline, baseline());
    }
    #[test]
    fn journal_write_failure_prevents_any_system_change() {
        assert!(matches!(
            reconcile(
                &mut None,
                Some(("Home", "direct")),
                baseline(),
                |_| panic!("no mutation before durable recovery record"),
                |_| Err("disk failed".into())
            ),
            Outcome::Failed(_)
        ));
    }
    #[test]
    fn missing_ssid_does_not_restore_an_existing_rule() {
        let state = entered();
        let mut rules = crossbow_core::model::NetworkScenes::default();
        rules.rules.insert("Home".into(), "direct".into());
        assert!(!can_restore_without_ssid(state.as_ref(), &rules));
        rules.rules.clear();
        assert!(can_restore_without_ssid(state.as_ref(), &rules));
    }

    #[test]
    fn journal_roundtrip_preserves_recovery_across_restart() {
        let state = entered().unwrap();
        let loaded: Entered = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        assert_eq!(loaded, state);
    }
}
