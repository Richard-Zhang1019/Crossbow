//! Dock 和系统退出请求也必须经过可取消的网络恢复。

use std::sync::{
    atomic::{AtomicU64, Ordering},
    OnceLock,
};
use std::time::Duration;

use objc2::{
    runtime::{AnyObject, ClassBuilder, Sel},
    sel, MainThreadMarker,
};
use objc2_app_kit::{NSApplication, NSApplicationTerminateReply};
use tauri::{AppHandle, Manager};

static APP: OnceLock<AppHandle> = OnceLock::new();
static PENDING: AtomicU64 = AtomicU64::new(0);
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

fn take_request(pending: &AtomicU64, request: u64) -> bool {
    pending
        .compare_exchange(request, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

/// 原生退出等待准备结果，保留系统注销或关机请求的后续处理。
extern "C" fn should_terminate(
    _: *mut AnyObject,
    _: Sel,
    _: *mut NSApplication,
) -> NSApplicationTerminateReply {
    if let Some(app) = APP.get() {
        if PENDING.load(Ordering::SeqCst) != 0 {
            // 上一轮仍在等交付且看门狗未超时：不再重复发起（request_process_exit
            // 的 begin()==false 本就是 no-op），维持 TerminateLater（#369 review P3）。
            return NSApplicationTerminateReply::TerminateLater;
        }
        if app
            .try_state::<crate::AppState>()
            .is_some_and(|state| state.exit.ready())
        {
            PENDING.store(0, Ordering::SeqCst);
            return NSApplicationTerminateReply::TerminateNow;
        }
        let request = NEXT_REQUEST.fetch_add(1, Ordering::SeqCst);
        PENDING.store(request, Ordering::SeqCst);
        crate::request_process_exit(app, None);
        spawn_reply_watchdog(app.clone(), request);
        return NSApplicationTerminateReply::TerminateLater;
    }
    NSApplicationTerminateReply::TerminateCancel
}

/// 交付丢失兜底（#559）：`should_terminate` 返回 TerminateLater 的前提是
/// 进行中的退出准备必然经 `finish_process_exit` 交付 reply。准备线程 panic
/// 或 `run_on_main_thread` 失败都会打破该前提——PENDING 永真、系统注销/
/// 关机永久挂起。看门狗超时后在主线程补交付 reply 放行。
fn spawn_reply_watchdog(app: AppHandle, request: u64) {
    std::thread::spawn(move || {
        // A monotonic delay and immutable generation prevent an old watchdog
        // from claiming a request started after cancellation.
        std::thread::sleep(REPLY_TIMEOUT);
        if PENDING.load(Ordering::SeqCst) != request {
            return;
        }
        let main = app.clone();
        if let Err(error) = app.run_on_main_thread(move || {
            if take_request(&PENDING, request) {
                eprintln!("shutdown watchdog: exit reply overdue; allowing system terminate");
                if let Some(state) = main.try_state::<crate::AppState>() {
                    state.exit.cancel();
                }
                let mtm = MainThreadMarker::new().expect("退出结果必须在主线程交付");
                NSApplication::sharedApplication(mtm).replyToApplicationShouldTerminate(true);
            }
        }) {
            eprintln!("shutdown watchdog: failed to deliver timeout reply: {error}");
        }
    });
}

/// 在主线程回复挂起的原生请求，恢复成功才允许系统继续退出。
pub(crate) fn complete(allow: bool) -> bool {
    if PENDING.swap(0, Ordering::SeqCst) == 0 {
        return false;
    }
    let mtm = MainThreadMarker::new().expect("退出结果必须在主线程交付");
    NSApplication::sharedApplication(mtm).replyToApplicationShouldTerminate(allow);
    true
}

/// 保留 Tao 的委托行为，仅在无新增字段的子类中补充原生退出入口。
pub(crate) fn install(app: &AppHandle) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("退出委托必须在主线程安装")?;
    let delegate = NSApplication::sharedApplication(mtm)
        .delegate()
        .ok_or("缺少 macOS 应用委托")?;
    let object: &AnyObject = (*delegate).as_ref();
    let mut builder = ClassBuilder::new(c"CrossbowQuitDelegate", object.class())
        .ok_or("无法创建 macOS 退出委托")?;
    // SAFETY: AppKit 的 applicationShouldTerminate: 签名与此函数一致。
    unsafe {
        builder.add_method(
            sel!(applicationShouldTerminate:),
            should_terminate
                as extern "C" fn(
                    *mut AnyObject,
                    Sel,
                    *mut NSApplication,
                ) -> NSApplicationTerminateReply,
        );
    }
    let class = builder.register();
    APP.set(app.clone()).map_err(|_| "macOS 退出委托已安装")?;
    // SAFETY: 主线程中把委托换为其无新增字段的子类；继承所有原有方法与布局。
    unsafe { AnyObject::set_class(object, class) };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_watchdog_cannot_complete_a_new_request() {
        let pending = AtomicU64::new(1);
        assert!(take_request(&pending, 1));
        pending.store(2, Ordering::SeqCst);
        assert!(!take_request(&pending, 1));
        assert_eq!(pending.load(Ordering::SeqCst), 2);
        assert!(take_request(&pending, 2));
        assert!(!take_request(&pending, 2));
    }
}
