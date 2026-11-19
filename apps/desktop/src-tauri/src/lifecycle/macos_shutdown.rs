//! Dock 和系统退出请求也必须经过可取消的网络恢复。

use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    OnceLock,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use objc2::{
    runtime::{AnyObject, ClassBuilder, Sel},
    sel, MainThreadMarker,
};
use objc2_app_kit::{NSApplication, NSApplicationTerminateReply};
use tauri::{AppHandle, Manager};

static APP: OnceLock<AppHandle> = OnceLock::new();
static PENDING: AtomicBool = AtomicBool::new(false);
/// 本次挂起请求的发起时刻（unix ms）：交付丢失看门狗的计时起点（#559）。
static PENDING_AT: AtomicU64 = AtomicU64::new(0);
/// 交付看门狗超时：超过即认为「进行中的那轮退出必然经 finish_process_exit
/// 交付 reply」的隐含前提已被打破（准备线程 panic / run_on_main_thread
/// 失败），主动放行系统关机。宁可残留代理（下次启动由
/// clear_stale_system_proxy_early 自愈）也不能挂死系统。
const REPLY_TIMEOUT_MS: u64 = 10_000;

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 原生退出等待准备结果，保留系统注销或关机请求的后续处理。
extern "C" fn should_terminate(
    _: *mut AnyObject,
    _: Sel,
    _: *mut NSApplication,
) -> NSApplicationTerminateReply {
    if let Some(app) = APP.get() {
        let first = !PENDING.swap(true, Ordering::SeqCst);
        if !first {
            // 上一轮仍在等交付且看门狗未超时：不再重复发起（request_process_exit
            // 的 begin()==false 本就是 no-op），维持 TerminateLater（#369 review P3）。
            return NSApplicationTerminateReply::TerminateLater;
        }
        if app
            .try_state::<crate::AppState>()
            .is_some_and(|state| state.exit.ready())
        {
            PENDING.store(false, Ordering::SeqCst);
            return NSApplicationTerminateReply::TerminateNow;
        }
        PENDING_AT.store(now_unix_ms(), Ordering::SeqCst);
        crate::request_process_exit(app, None);
        spawn_reply_watchdog(app.clone());
        return NSApplicationTerminateReply::TerminateLater;
    }
    NSApplicationTerminateReply::TerminateCancel
}

/// 交付丢失兜底（#559）：`should_terminate` 返回 TerminateLater 的前提是
/// 进行中的退出准备必然经 `finish_process_exit` 交付 reply。准备线程 panic
/// 或 `run_on_main_thread` 失败都会打破该前提——PENDING 永真、系统注销/
/// 关机永久挂起。看门狗超时后在主线程补交付 reply 放行。
fn spawn_reply_watchdog(app: AppHandle) {
    std::thread::spawn(move || {
        let deadline = PENDING_AT.load(Ordering::SeqCst) + REPLY_TIMEOUT_MS;
        while now_unix_ms() < deadline {
            if !PENDING.load(Ordering::SeqCst) {
                return; // 已正常交付
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        if !PENDING.load(Ordering::SeqCst) {
            return;
        }
        eprintln!(
            "shutdown watchdog: exit reply overdue ({REPLY_TIMEOUT_MS}ms); allowing system terminate, leftover proxy state heals on next launch"
        );
        let main = app.clone();
        let _ = app.run_on_main_thread(move || {
            if PENDING.swap(false, Ordering::SeqCst) {
                // ExitState 复位：那轮准备已丢失，不再占用退出通道。
                if let Some(state) = main.try_state::<crate::AppState>() {
                    state.exit.cancel();
                }
                let mtm = MainThreadMarker::new().expect("退出结果必须在主线程交付");
                NSApplication::sharedApplication(mtm).replyToApplicationShouldTerminate(true);
            }
        });
    });
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
