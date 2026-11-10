//! 可取消的退出准备：恢复网络成功后才停止内核，失败后允许重试。

use std::sync::atomic::{AtomicU8, Ordering};

const IDLE: u8 = 0;
const PREPARING: u8 = 1;
const READY: u8 = 2;

#[derive(Default)]
pub(crate) struct ExitState(AtomicU8);

impl ExitState {
    /// 同一时刻只启动一个退出准备任务。
    pub(crate) fn begin(&self) -> bool {
        self.0
            .compare_exchange(IDLE, PREPARING, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// 只允许准备完成后的第二次退出请求真正结束进程。
    pub(crate) fn ready(&self) -> bool {
        self.0.load(Ordering::SeqCst) == READY
    }

    /// 仅在进入内核事务时检查，不打断已经取得操作锁的事务。
    pub(crate) fn ensure_open(&self) -> Result<(), String> {
        if self.0.load(Ordering::SeqCst) == IDLE {
            Ok(())
        } else {
            Err("应用正在准备退出，请在退出取消后重试".into())
        }
    }

    /// 恢复失败或安装器启动失败时，重新开放操作与退出重试。
    pub(crate) fn cancel(&self) {
        self.0.store(IDLE, Ordering::SeqCst);
    }

    /// 普通退出先恢复再停止；更新交接跳过两者，成功后才释放会话。
    pub(crate) fn finish(
        &self,
        preserve_core: bool,
        restore: impl FnOnce() -> Result<(), String>,
        stop: impl FnOnce() -> Result<(), String>,
        release_session: impl FnOnce(),
    ) -> Result<(), String> {
        let result = if preserve_core {
            Ok(())
        } else {
            restore().and_then(|()| stop())
        };
        if result.is_ok() {
            release_session();
        }
        if result.is_ok() {
            self.0.store(READY, Ordering::SeqCst);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// 授权取消时监听仍在，重试成功后才可放行退出。
    #[test]
    fn failed_restore_keeps_listener_and_allows_retry() {
        let exit = ExitState::default();
        let listener = RefCell::new(Some(std::net::TcpListener::bind("127.0.0.1:0").unwrap()));
        let address = listener.borrow().as_ref().unwrap().local_addr().unwrap();
        assert!(exit.begin());
        assert!(exit
            .finish(
                false,
                || Err("authorization cancelled".into()),
                || {
                    listener.borrow_mut().take();
                    Ok(())
                },
                || panic!("恢复失败不能释放会话"),
            )
            .is_err());
        assert!(std::net::TcpStream::connect(address).is_ok());
        assert!(!exit.ready());
        assert!(!exit.begin(), "错误结果交付完成前不能开始另一轮退出");
        exit.cancel();
        assert!(exit.ensure_open().is_ok());
        assert!(exit.begin());
        exit.finish(
            false,
            || Ok(()),
            || {
                listener.borrow_mut().take();
                Ok(())
            },
            || {},
        )
        .unwrap();
        assert!(exit.ready());
        assert!(std::net::TcpStream::connect(address).is_err());
    }

    /// 重复退出只产生一次准备，且恢复必须先于停止。
    #[test]
    fn duplicate_exit_is_coalesced_and_restore_precedes_stop() {
        let exit = ExitState::default();
        let events = RefCell::new(Vec::new());
        assert!(exit.begin());
        assert!(!exit.begin());
        assert!(exit.ensure_open().is_err());
        exit.finish(
            false,
            || {
                events.borrow_mut().push("restore");
                Ok(())
            },
            || {
                events.borrow_mut().push("stop");
                Ok(())
            },
            || events.borrow_mut().push("release"),
        )
        .unwrap();
        assert_eq!(*events.borrow(), ["restore", "stop", "release"]);
        assert!(!exit.begin());
        assert!(exit.ready());
    }

    /// 成功交接更新时保留内核与代理，只释放旧 GUI 的会话资源。
    #[test]
    fn update_handoff_skips_network_and_core_cleanup() {
        let exit = ExitState::default();
        let released = std::cell::Cell::new(false);
        assert!(exit.begin());
        exit.finish(
            true,
            || panic!("不能恢复接力中的代理"),
            || panic!("不能停止接力中的内核"),
            || released.set(true),
        )
        .unwrap();
        assert!(released.get());
        assert!(exit.ready());
    }
}
