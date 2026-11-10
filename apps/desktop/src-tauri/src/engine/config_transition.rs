//! 配置变更走哪条路。
//!
//! 计划选择与事务执行顺序，不依赖具体进程 API。调用方据此选择：只落盘、mihomo `PUT /configs?force=true`、
//! 或停掉再拉起。TUN 开关不走这里，它要换特权进程，仍由各平台 `toggle_tun` 处理。

use crossbow_core::Engine;

/// 一次变更对正在运行的内核该怎么生效。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    /// 内核没在跑。新状态写入存储即可，不要去停一个不存在的进程。
    PersistOnly,
    /// 同一 mihomo 进程重载配置。系统代理和监听端口保持不动。
    HotReload,
    /// 进程身份或监听变了，必须停再启。
    Restart,
}

/// 决定热重载是否安全的现场条件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveCore {
    pub running: bool,
    pub mihomo: bool,
    pub controller: bool,
    /// 已有一份可回滚的运行配置。没有它，热重载失败后无法把旧文件写回去。
    pub cached_config: bool,
    /// Windows SYSTEM 内核使用服务校验后的独立配置副本，不能重载 GUI 路径。
    pub service_managed: bool,
}

/// 订阅、覆写、切换当前档案：内容变了，进程身份没变。
///
/// 热重载要求三件事同时成立：mihomo、控制器还在、手里有旧配置可回滚。
/// sing-box 的 clash_api 不能可靠重载，缺控制器或旧配置时只能停再启。
pub fn plan_content(live: LiveCore) -> Plan {
    if !live.running {
        return Plan::PersistOnly;
    }
    if live.mihomo && live.controller && live.cached_config && !live.service_managed {
        Plan::HotReload
    } else {
        Plan::Restart
    }
}

/// 换引擎、改混合端口、改局域网绑定。监听或二进制变了，热重载盖不住。
pub fn plan_identity(live: LiveCore) -> Plan {
    if live.running {
        Plan::Restart
    } else {
        Plan::PersistOnly
    }
}

/// 引擎、端口、局域网、TUN 任一变化都不是「只改订阅内容」。
pub fn needs_new_process(
    before_engine: Engine,
    after_engine: Engine,
    before_port: u16,
    after_port: u16,
    before_lan: bool,
    after_lan: bool,
    before_tun: bool,
    after_tun: bool,
) -> bool {
    before_engine != after_engine
        || before_port != after_port
        || before_lan != after_lan
        || before_tun != after_tun
}

/// 事务拥有失败恢复权时，状态回调不得同步重入 core_op。
/// RAII 确保普通错误返回和 panic 都会归还恢复权。
pub struct RecoveryOwner<'a>(&'a std::sync::atomic::AtomicBool);
impl<'a> RecoveryOwner<'a> {
    pub fn enter(flag: &'a std::sync::atomic::AtomicBool) -> Self {
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        Self(flag)
    }
}
impl Drop for RecoveryOwner<'_> {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// 执行与持久化共用同一失败出口。恢复失败不能掩盖原始错误。
pub trait Transaction {
    fn apply(&mut self) -> Result<(), String>;
    fn persist(&mut self) -> Result<(), String>;
    fn rollback(&mut self) -> Result<(), String>;
}
pub fn execute(transaction: &mut impl Transaction) -> Result<(), String> {
    match transaction.apply().and_then(|()| transaction.persist()) {
        Ok(()) => Ok(()),
        Err(error) => match transaction.rollback() {
            Ok(()) => Err(format!("切换失败，已恢复之前的状态：{error}")),
            Err(recovery) => Err(format!("切换失败：{error}；恢复未完成：{recovery}")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(running: bool, mihomo: bool, controller: bool, cached_config: bool) -> LiveCore {
        LiveCore {
            running,
            mihomo,
            controller,
            cached_config,
            service_managed: false,
        }
    }

    #[test]
    fn service_managed_core_never_reloads_the_gui_path() {
        let mut managed = live(true, true, true, true);
        managed.service_managed = true;
        assert_eq!(plan_content(managed), Plan::Restart);
    }

    #[derive(Default)]
    struct Faults {
        events: Vec<&'static str>,
        apply_error: bool,
        save_error: bool,
        rollback_error: bool,
        memory: u16,
        disk: u16,
        runtime: u16,
        proxy: Option<u16>,
        old_proxy: Option<u16>,
    }
    impl Transaction for Faults {
        fn apply(&mut self) -> Result<(), String> {
            self.events.push("apply");
            self.memory = 7898;
            self.runtime = 7898;
            self.proxy = self.old_proxy.map(|_| 7898);
            if self.apply_error {
                Err("core failed".into())
            } else {
                Ok(())
            }
        }
        fn persist(&mut self) -> Result<(), String> {
            self.events.push("persist");
            if self.save_error {
                return Err("disk failed".into());
            }
            self.disk = self.memory;
            Ok(())
        }
        fn rollback(&mut self) -> Result<(), String> {
            self.events.push("rollback");
            self.memory = 7897;
            self.disk = 7897;
            self.runtime = 7897;
            self.proxy = self.old_proxy;
            if self.rollback_error {
                Err("restore failed".into())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn successful_transaction_persists_after_apply_without_rollback() {
        let mut tx = Faults::default();
        execute(&mut tx).unwrap();
        assert_eq!(tx.events, ["apply", "persist"]);
        assert_eq!((tx.memory, tx.disk, tx.runtime), (7898, 7898, 7898));
    }

    #[test]
    fn failed_start_does_not_commit_and_restores_original_proxy_state() {
        for old_proxy in [None, Some(7897)] {
            let mut tx = Faults {
                apply_error: true,
                old_proxy,
                ..Faults::default()
            };
            assert!(execute(&mut tx).unwrap_err().contains("core failed"));
            assert_eq!(tx.events, ["apply", "rollback"]);
            assert_eq!((tx.memory, tx.disk, tx.runtime), (7897, 7897, 7897));
            assert_eq!(tx.proxy, old_proxy);
        }
    }

    #[test]
    fn persist_failure_rolls_back_the_already_applied_configuration() {
        let mut tx = Faults {
            save_error: true,
            old_proxy: Some(7897),
            ..Faults::default()
        };
        assert!(execute(&mut tx).unwrap_err().contains("disk failed"));
        assert_eq!(tx.events, ["apply", "persist", "rollback"]);
        assert_eq!((tx.memory, tx.disk, tx.runtime), (7897, 7897, 7897));
        assert_eq!(tx.proxy, Some(7897));
    }

    #[test]
    fn rollback_failure_retains_both_errors_without_claiming_recovery() {
        for apply_error in [true, false] {
            let mut tx = Faults {
                apply_error,
                save_error: true,
                rollback_error: true,
                ..Faults::default()
            };
            let error = execute(&mut tx).unwrap_err();
            assert!(error.contains(if apply_error {
                "core failed"
            } else {
                "disk failed"
            }));
            assert!(error.contains("restore failed"));
            assert!(!error.contains("已恢复"));
        }
    }

    #[test]
    fn transaction_recovery_ownership_is_visible_to_callbacks_and_released() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let flag = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let _owner = RecoveryOwner::enter(&flag);
            // A status callback on the watch thread must see transaction-owned
            // recovery, so it can return instead of entering core_op again.
            scope
                .spawn(|| assert!(flag.load(Ordering::SeqCst)))
                .join()
                .unwrap();
        });
        assert!(!flag.load(Ordering::SeqCst));
        let _ = std::panic::catch_unwind(|| {
            let _owner = RecoveryOwner::enter(&flag);
            panic!("injected transition panic");
        });
        assert!(!flag.load(Ordering::SeqCst));
    }

    #[test]
    fn content_change_reloads_a_running_mihomo_with_a_cached_config() {
        assert_eq!(plan_content(live(true, true, true, true)), Plan::HotReload);
    }

    #[test]
    fn content_change_does_not_touch_a_stopped_core() {
        assert_eq!(
            plan_content(live(false, true, false, false)),
            Plan::PersistOnly
        );
    }

    #[test]
    fn content_change_restarts_when_hot_reload_cannot_roll_back() {
        assert_eq!(plan_content(live(true, true, true, false)), Plan::Restart);
        assert_eq!(plan_content(live(true, true, false, true)), Plan::Restart);
        assert_eq!(plan_content(live(true, false, true, true)), Plan::Restart);
    }

    #[test]
    fn identity_change_restarts_only_when_the_core_is_running() {
        assert_eq!(plan_identity(live(true, true, true, true)), Plan::Restart);
        assert_eq!(
            plan_identity(live(false, false, false, false)),
            Plan::PersistOnly
        );
    }

    #[test]
    fn engine_port_lan_or_tun_change_needs_a_new_process() {
        assert!(needs_new_process(
            Engine::Mihomo,
            Engine::SingBox,
            7897,
            7897,
            false,
            false,
            false,
            false
        ));
        assert!(needs_new_process(
            Engine::Mihomo,
            Engine::Mihomo,
            7897,
            7898,
            false,
            false,
            false,
            false
        ));
        assert!(needs_new_process(
            Engine::Mihomo,
            Engine::Mihomo,
            7897,
            7897,
            false,
            true,
            false,
            false
        ));
        assert!(needs_new_process(
            Engine::Mihomo,
            Engine::Mihomo,
            7897,
            7897,
            false,
            false,
            false,
            true
        ));
        assert!(!needs_new_process(
            Engine::Mihomo,
            Engine::Mihomo,
            7897,
            7897,
            false,
            false,
            true,
            true
        ));
    }
}
