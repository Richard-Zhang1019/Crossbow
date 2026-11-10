//! 后台线程只允许已经具备权限的操作，不能唤起管理员授权窗口。

use std::cell::Cell;

thread_local! {
    static INTERACTIVE: Cell<bool> = const { Cell::new(true) };
}

pub(crate) struct BackgroundGuard(bool, std::marker::PhantomData<std::rc::Rc<()>>);

impl BackgroundGuard {
    /// 只约束当前自检线程，不影响用户操作所在的线程。
    pub(crate) fn enter() -> Self {
        Self(
            INTERACTIVE.with(|flag| flag.replace(false)),
            std::marker::PhantomData,
        )
    }
}

impl Drop for BackgroundGuard {
    fn drop(&mut self) {
        INTERACTIVE.with(|flag| flag.set(self.0));
    }
}

/// 所有交互提权入口共用此检查，后台失败须交给用户手动恢复。
pub(crate) fn require_interactive() -> Result<(), String> {
    if INTERACTIVE.with(Cell::get) {
        Ok(())
    } else {
        Err("后台操作需要管理员授权，请手动操作系统代理或 TUN 开关".into())
    }
}

/// 监视线程无论由谁启动都属于自主维护，不能继承用户操作的弹窗权限。
pub(crate) fn spawn_background<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> std::thread::JoinHandle<T> {
    std::thread::spawn(move || {
        let _background = BackgroundGuard::enter();
        work()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 嵌套后台操作不应提前恢复授权权限，其他线程的用户操作不受影响。
    #[test]
    fn background_authorization_is_scoped_and_thread_local() {
        assert!(require_interactive().is_ok());
        {
            let _background = BackgroundGuard::enter();
            assert!(require_interactive().is_err());
            {
                let _nested = BackgroundGuard::enter();
            }
            assert!(require_interactive().is_err());
            assert!(std::thread::spawn(require_interactive)
                .join()
                .unwrap()
                .is_ok());
        }
        assert!(require_interactive().is_ok());
    }

    /// 子监视线程和继续派生的监视线程都不得重新打开管理员授权。
    #[test]
    fn background_watchers_remain_noninteractive_across_spawns() {
        assert!(spawn_background(|| {
            assert!(require_interactive().is_err());
            spawn_background(require_interactive).join().unwrap()
        })
        .join()
        .unwrap()
        .is_err());
        assert!(require_interactive().is_ok());
    }
}
