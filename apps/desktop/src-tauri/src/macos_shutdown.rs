//! Dock 和系统退出请求也必须经过可取消的网络恢复。

use std::sync::{
    atomic::{AtomicBool, Ordering},
    OnceLock,
};

use objc2::{
    runtime::{AnyObject, ClassBuilder, Sel},
    sel, MainThreadMarker,
};
use objc2_app_kit::{NSApplication, NSApplicationTerminateReply};
use tauri::{AppHandle, Manager};

static APP: OnceLock<AppHandle> = OnceLock::new();
static PENDING: AtomicBool = AtomicBool::new(false);

/// 原生退出等待准备结果，保留系统注销或关机请求的后续处理。
extern "C" fn should_terminate(
    _: *mut AnyObject,
    _: Sel,
    _: *mut NSApplication,
) -> NSApplicationTerminateReply {
    if let Some(app) = APP.get() {
        PENDING.store(true, Ordering::SeqCst);
        if app
            .try_state::<crate::AppState>()
            .is_some_and(|state| state.exit.ready())
        {
            PENDING.store(false, Ordering::SeqCst);
            return NSApplicationTerminateReply::TerminateNow;
        }
        crate::request_process_exit(app, None);
        return NSApplicationTerminateReply::TerminateLater;
    }
    NSApplicationTerminateReply::TerminateCancel
}

/// 在主线程回复挂起的原生请求，恢复成功才允许系统继续退出。
pub(crate) fn complete(allow: bool) -> bool {
    if !PENDING.swap(false, Ordering::SeqCst) {
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
