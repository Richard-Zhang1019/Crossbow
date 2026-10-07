//! macOS 菜单栏实时速率：NSStatusItem 富文本双行标题（借鉴 Clash Verge Rev 的
//! 公开技术方案，实现独立编写）。
//!
//! 系统菜单栏高 24pt，默认字号塞不下两行；方案：9pt 等宽字体 + 固定 10pt 行高 +
//! 精确基线偏移把两行垂直居中在按钮内，并按内容宽度设定 status item 长度以避免
//! 每秒抖动。仅 macOS，其余平台为空操作。

use tauri::AppHandle;

use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::NSAttributedStringNSStringDrawing as _;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSBaselineOffsetAttributeName, NSColor, NSFont, NSFontAttributeName,
    NSFontWeightMedium, NSForegroundColorAttributeName, NSLineBreakMode,
    NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSStatusBarButton,
    NSStatusItem, NSTextAlignment,
};
use objc2_foundation::{NSAttributedString, NSAttributedStringKey, NSDictionary, NSNumber, NSString};

const FONT_SIZE: f64 = 9.5;
const LINE_HEIGHT: f64 = 10.0;
/// 图标占位 + 系统内边距（Tauri 图标 18pt）。
const EXTRA_WIDTH: f64 = 30.0;
/// 短值（如 ↓0B↑0B）防抖的最小宽度。
const MIN_LENGTH: f64 = 56.0;
const NS_VARIABLE_LENGTH: f64 = -1.0;

thread_local! {
    /// 内容不变时跳过重设（每秒一次的 tick 大多数帧是无变化的）。
    static LAST: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// 速率文本：上行在上、下行在下；每行 `{:>6}` 填充——等宽字体下
/// 字符数恒定 ⇒ 托盘宽度恒定，不随数字变化抖动（Verge 同款方案）。
fn speed_text(up: u64, down: u64) -> String {
    format!("{:>6}\n{:>6}", speed_str(up), speed_str(down))
}

/// 紧凑速率串，输出保证 ≤6 字符：<1000 → `999B/s`；否则选单位让数值
/// 落在 1000 以内，<9.95 一位小数（`9.9K/s`），其余取整（`123M/s`）。
fn speed_str(bps: u64) -> String {
    const UNITS: [&str; 5] = ["B/s", "K/s", "M/s", "G/s", "T/s"];
    if bps < 1000 {
        return format!("{bps}B/s");
    }
    let mut u = (u64::ilog2(bps) / 10) as usize;
    u = u.min(UNITS.len() - 1);
    let mut v = bps as f64 / 1024f64.powi(u as i32);
    if v.round() >= 1000.0 && u < UNITS.len() - 1 {
        u += 1;
        v = bps as f64 / 1024f64.powi(u as i32);
    }
    if v < 9.95 {
        format!("{v:.1}{}", UNITS[u])
    } else {
        format!("{:.0}{}", v.round(), UNITS[u])
    }
}

/// 按钮高度构建属性字典：等宽小字 + 系统文字色 + 固定行高 + 垂直居中基线。
fn build_attrs(button_height: f64) -> Retained<NSDictionary<NSAttributedStringKey, AnyObject>> {
    unsafe {
        // Medium 字重：9.5pt 细体在暗色菜单栏上光学偏灰，加粗一档提亮
        let font = NSFont::monospacedSystemFontOfSize_weight(FONT_SIZE, NSFontWeightMedium);
        let color = NSColor::controlTextColor();
        let para = NSMutableParagraphStyle::new();
        para.setAlignment(NSTextAlignment::Right);
        para.setLineBreakMode(NSLineBreakMode::ByClipping);
        para.setMinimumLineHeight(LINE_HEIGHT);
        para.setMaximumLineHeight(LINE_HEIGHT);
        // 基线下移把两行文本块在按钮内垂直居中
        let glyph_height = font.ascender() - font.descender();
        let free = LINE_HEIGHT * 2.0 - button_height;
        let baseline = NSNumber::new_f64(-(glyph_height / 3.0) + free / 2.0);

        let keys: &[&NSAttributedStringKey] = &[
            NSFontAttributeName,
            NSForegroundColorAttributeName,
            NSParagraphStyleAttributeName,
            NSBaselineOffsetAttributeName,
        ];
        let values: &[&AnyObject] = &[&font, &color, &para, &baseline];
        NSDictionary::from_slices(keys, values)
    }
}

/// 主线程应用富文本标题（with_inner_tray_icon 的回调在主线程执行）。
fn apply(status_item: &NSStatusItem, text: &str, show_speed: bool) {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("tray_speed: not on main thread, skip");
        return;
    };
    let Some(button): Option<Retained<NSStatusBarButton>> =
        status_item.button(mtm)
    else {
        return;
    };
    let ns_text = NSString::from_str(text);
    let attr_str = if show_speed {
        let attrs = build_attrs(button.bounds().size.height);
        unsafe { NSAttributedString::new_with_attributes(&ns_text, &attrs) }
    } else {
        NSAttributedString::from_nsstring(&ns_text)
    };
    if show_speed {
        let width = attr_str.size().width.ceil() + EXTRA_WIDTH;
        let length = width.max(MIN_LENGTH);
        if status_item.length() != length {
            status_item.setLength(length);
        }
    }
    button.setAttributedTitle(&attr_str);
}

/// 更新托盘速率双行显示。
pub fn set_speed(handle: &AppHandle, up: u64, down: u64) {
    let text = speed_text(up, down);
    LAST.with(|last| {
        if last.borrow().as_str() == text {
            return;
        }
        *last.borrow_mut() = text.clone();
        let text_for_apply = text.clone();
        let _ = handle.tray_by_id_or_first(move |item| {
            apply(item, &text_for_apply, true);
        });
    });
}

/// 清空速率显示（内核停止等场景），status item 宽度恢复自适应。
pub fn clear(handle: &AppHandle) {
    LAST.with(|last| *last.borrow_mut() = String::new());
    let _ = handle.tray_by_id_or_first(|item| {
        item.setLength(NS_VARIABLE_LENGTH);
        apply(item, "", false);
    });
}

/// 取应用唯一的托盘（我们不建多个），在主线程上执行操作。
trait TrayExt {
    fn tray_by_id_or_first(&self, f: impl FnOnce(&NSStatusItem) + Send + 'static) -> Result<(), String>;
}

impl TrayExt for AppHandle {
    fn tray_by_id_or_first(&self, f: impl FnOnce(&NSStatusItem) + Send + 'static) -> Result<(), String> {
        use tauri::Manager;
        let state = self.state::<crate::AppState>();
        let tray = state.tray.lock().unwrap().clone();
        let Some(tray) = tray else {
            return Err("no tray".into());
        };
        tray.with_inner_tray_icon(move |inner| {
            if let Some(item) = inner.ns_status_item() {
                f(&item);
            }
        })
        .map_err(|e| e.to_string())
    }
}
