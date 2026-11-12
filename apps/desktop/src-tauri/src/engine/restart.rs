//! 服务托管内核的重启前清理。
//!
//! GUI 杀不掉 SYSTEM 进程，也拒绝给没有二进制身份的活 PID 发信号。
//! 因此服务托管租约必须先请服务停，服务确认失败后才允许走进程信号。
//!
//! 这是这类顺序问题的标准回归：新场景加进 `cleanup_cases`，生产路径只调用
//! `cleanup_before_restart`，不要在 `start_attempt` 里再写一套顺序。

use std::path::Path;

pub(crate) const RESTART_BLOCKED: &str = "无法确认上一次内核已退出，已中止新启动";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cleanup {
    /// 没有服务托管租约，或服务没能停掉但进程信号仍然允许。调用方继续 `kill_child`。
    Continue,
    /// 服务已确认停掉。调用方清簿记，不能再对这个 PID 发信号。
    Released,
}

/// `has_local_child` 为真时，进程句柄在本进程，不把普通内核误当成服务租约。
/// 服务托管的形状是：只有 `external_pid`，没有本地 `Child`。
pub(crate) fn cleanup_before_restart(
    has_local_child: bool,
    external_pid: Option<u32>,
    external_binary: Option<&Path>,
    service_stop: impl FnOnce() -> Result<(), String>,
    pid_alive: impl Fn() -> bool,
) -> Result<Cleanup, String> {
    if external_pid.is_none() || has_local_child {
        return Ok(Cleanup::Continue);
    }
    // 服务可能停的是另一份内核。只有被跟踪的 PID 已经退出，才能清掉这份租约。
    let stopped = service_stop().is_ok();
    let alive = pid_alive();
    if stopped && !alive {
        return Ok(Cleanup::Released);
    }
    if external_binary.is_none() && alive {
        return Err(RESTART_BLOCKED.into());
    }
    Ok(Cleanup::Continue)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Case {
        name: &'static str,
        has_local_child: bool,
        external_pid: Option<u32>,
        has_binary: bool,
        alive_after_stop: bool,
        service_ok: bool,
        service_called: bool,
        expect: Result<Cleanup, ()>,
    }

    fn cleanup_cases() -> Vec<Case> {
        vec![
            Case {
                name: "本地子进程不请服务停",
                has_local_child: true,
                external_pid: None,
                has_binary: false,
                alive_after_stop: true,
                service_ok: false,
                service_called: false,
                expect: Ok(Cleanup::Continue),
            },
            Case {
                name: "没有外部租约不请服务停",
                has_local_child: false,
                external_pid: None,
                has_binary: false,
                alive_after_stop: false,
                service_ok: false,
                service_called: false,
                expect: Ok(Cleanup::Continue),
            },
            Case {
                name: "服务停掉被跟踪的 PID 后释放租约",
                has_local_child: false,
                external_pid: Some(42),
                has_binary: false,
                alive_after_stop: false,
                service_ok: true,
                service_called: true,
                expect: Ok(Cleanup::Released),
            },
            Case {
                name: "服务停的是另一份内核时不能清掉仍活着的租约",
                has_local_child: false,
                external_pid: Some(42),
                has_binary: false,
                alive_after_stop: true,
                service_ok: true,
                service_called: true,
                expect: Err(()),
            },
            Case {
                name: "活着的未校验 PID 在服务停失败后拒绝发信号",
                has_local_child: false,
                external_pid: Some(42),
                has_binary: false,
                alive_after_stop: true,
                service_ok: false,
                service_called: true,
                expect: Err(()),
            },
            Case {
                name: "已经退出的未校验 PID 不因服务停失败而卡住",
                has_local_child: false,
                external_pid: Some(42),
                has_binary: false,
                alive_after_stop: false,
                service_ok: false,
                service_called: true,
                expect: Ok(Cleanup::Continue),
            },
            Case {
                name: "有身份的活 PID 在服务停失败后仍允许发信号",
                has_local_child: false,
                external_pid: Some(42),
                has_binary: true,
                alive_after_stop: true,
                service_ok: false,
                service_called: true,
                expect: Ok(Cleanup::Continue),
            },
        ]
    }

    #[test]
    fn service_hosted_restart_cleanup_follows_the_scenario_table() {
        let binary = PathBuf::from("mihomo.exe");
        for case in cleanup_cases() {
            let calls = AtomicUsize::new(0);
            let result = cleanup_before_restart(
                case.has_local_child,
                case.external_pid,
                case.has_binary.then_some(binary.as_path()),
                || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    if case.service_ok {
                        Ok(())
                    } else {
                        Err("no core".into())
                    }
                },
                || case.alive_after_stop,
            );
            assert_eq!(
                calls.load(Ordering::SeqCst) == 1,
                case.service_called,
                "{}",
                case.name
            );
            match case.expect {
                Ok(cleanup) => assert_eq!(result.unwrap(), cleanup, "{}", case.name),
                Err(()) => {
                    let err = result.expect_err(case.name);
                    assert!(err.contains(RESTART_BLOCKED), "{}: {err}", case.name);
                }
            }
        }
    }
}
