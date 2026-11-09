//! Windows 会话结束（关机/注销）时的兜底恢复（#334 review P3）。
//!
//! Tauri 的 `ExitRequested` 在 Windows 会话结束路径上不被触发：系统只给
//! 每个 top-level 窗口发 `WM_ENDSESSION`，数秒后即终止进程——来不及走
//! 完整的网络恢复。子类化主窗口，在会话真正结束（`wParam != 0`）时同步
//! 执行最快的系统代理还原（注册表写 + 刷新广播，毫秒级），保证重启后
//! 浏览器不会指向一个已死的代理端口（#356 同源场景）。内核进程交给
//! 系统终止，无需处理。

use std::sync::{
    atomic::{AtomicBool, Ordering},
    OnceLock,
};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{WM_ENDSESSION, WM_QUERYENDSESSION};

static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
static RESTORED: AtomicBool = AtomicBool::new(false);

const SUBCLASS_ID: usize = 0x00C7_05_5B;

unsafe extern "system" fn session_end_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    // WM_QUERYENDSESSION 放行默认处理（返回 TRUE 表示同意会话结束）；
    // 只在 WM_ENDSESSION 且会话确实要结束时做还原，一次性。
    if msg == WM_ENDSESSION && wparam != 0 && !RESTORED.swap(true, Ordering::SeqCst) {
        if let Some(app) = APP.get() {
            if let Some(state) = app.try_state::<crate::AppState>() {
                if state.sysproxy.status().enabled {
                    if let Err(error) = state.sysproxy.disable() {
                        eprintln!("session-end proxy restore failed: {error}");
                    }
                }
            }
        }
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

/// 主窗口创建后安装子类；dwRefData 不挂数据，状态经静态 APP 取得，
/// 避免悬垂指针。
pub(crate) fn install(app: &tauri::AppHandle, hwnd: HWND) {
    let _ = APP.set(app.clone());
    unsafe {
        if SetWindowSubclass(hwnd, Some(session_end_proc), SUBCLASS_ID, 0) == 0 {
            eprintln!("session-end subclass install failed");
        }
    }
}
