//! macOS 菜单栏实时速率：NSStatusItem 富文本双行标题（借鉴 Clash Verge Rev 的
//! 公开技术方案，实现独立编写）。
//!
//! 系统菜单栏高 24pt，默认字号塞不下两行；方案：9pt 等宽字体 + 固定 10pt 行高 +
//! 精确基线偏移把两行垂直居中在按钮内，并按内容宽度设定 status item 长度以避免
//! 每秒抖动。仅 macOS，其余平台为空操作。

use tauri::{AppHandle, Manager};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AllocAnyThread, MainThreadMarker};
use objc2_app_kit::{NSAttributedStringNSStringDrawing as _, NSStringDrawing as _};
use objc2_app_kit::{
    NSBaselineOffsetAttributeName, NSBezierPath, NSColor, NSFont, NSFontAttributeName,
    NSFontWeightMedium, NSFontWeightRegular, NSForegroundColorAttributeName, NSImage,
    NSLineBreakMode, NSMenu, NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSEvent,
    NSEventModifierFlags, NSEventType, NSStatusBarButton, NSStatusItem, NSTextAlignment,
    NSTextAttachment,
};
use objc2_app_kit::NSAttachmentAttributeName;
use objc2_foundation::{NSArray, NSAttributedString, NSAttributedStringKey, NSDictionary, NSNumber, NSString};
use objc2_foundation::NSMutableAttributedString;

const FONT_SIZE: f64 = 9.5;
const LINE_HEIGHT: f64 = 10.0;
// 图标占位 + 系统内边距（Tauri 图标 18pt）。
const EXTRA_WIDTH: f64 = 30.0;
// 短值（如 ↓0B↑0B）防抖的最小宽度。
const MIN_LENGTH: f64 = 56.0;
const NS_VARIABLE_LENGTH: f64 = -1.0;

thread_local! {
    /// 内容不变时跳过重设（每秒一次的 tick 大多数帧是无变化的）。
    static LAST: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

// 速率文本：上行在上、下行在下；每行 `{:>6}` 填充——等宽字体下
// 字符数恒定 ⇒ 托盘宽度恒定，不随数字变化抖动（Verge 同款方案）。
fn speed_text(up: u64, down: u64) -> String {
    format!("{:>6}\n{:>6}", speed_str(up), speed_str(down))
}

// 紧凑速率串，输出保证 ≤6 字符：<1000 → `999B/s`；否则选单位让数值
// 落在 1000 以内，<9.95 一位小数（`9.9K/s`），其余取整（`123M/s`）。
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

// 按钮高度构建属性字典：等宽小字 + 系统文字色 + 固定行高 + 垂直居中基线。
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

// 主线程应用富文本标题（with_inner_tray_icon 的回调在主线程执行）。
fn apply(status_item: &NSStatusItem, text: &str, show_speed: bool) {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("tray_speed: not on main thread, skip");
        return;
    };
    let Some(button): Option<Retained<NSStatusBarButton>> = status_item.button(mtm) else {
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

// 更新托盘速率双行显示。
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

// 清空速率显示（内核停止等场景），status item 宽度恢复自适应。
pub fn clear(handle: &AppHandle) {
    LAST.with(|last| *last.borrow_mut() = String::new());
    let _ = handle.tray_by_id_or_first(|item| {
        item.setLength(NS_VARIABLE_LENGTH);
        apply(item, "", false);
    });
}

// 取应用唯一的托盘（我们不建多个），在主线程上执行操作。
trait TrayExt {
    fn tray_by_id_or_first(
        &self,
        f: impl FnOnce(&NSStatusItem) + Send + 'static,
    ) -> Result<(), String>;
}

impl TrayExt for AppHandle {
    fn tray_by_id_or_first(
        &self,
        f: impl FnOnce(&NSStatusItem) + Send + 'static,
    ) -> Result<(), String> {
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


// ---------- 托盘节点子菜单：固定宽度、右对齐测速列 ----------

// 近似显示宽度：CJK/全角/emoji 记 2，其余记 1。
pub fn display_width(s: &str) -> usize {
    s.chars()
        .map(|c| {
            let cp = c as u32;
            if cp >= 0x1100
                && ((0x1100..=0x115F).contains(&cp)
                    || (0x2E80..=0xA4CF).contains(&cp)
                    || (0xAC00..=0xD7A3).contains(&cp)
                    || (0xF900..=0xFAFF).contains(&cp)
                    || (0xFE30..=0xFE4F).contains(&cp)
                    || (0xFF00..=0xFF60).contains(&cp)
                    || (0xFFE0..=0xFFE6).contains(&cp)
                    || cp >= 0x1F300)
            {
                2
            } else {
                1
            }
        })
        .sum()
}

// 递归遍历托盘主菜单与子菜单，对测速可用态、节点富文本和勾选态做原地更新。
// 不调用 set_menu——菜单打开时刷新可见、不关闭。

// 单个节点行数据（lib.rs 组装，tray_speed 负责原生渲染与原地刷新）。
#[derive(Clone)]
pub struct TrayNodeRow {
    /// 节点名
    pub name: String,
    /// 延迟文本：`448 ms` / `失败` / `—`
    pub badge: String,
    /// 延迟毫秒数（失败或未测为 None）
    pub ms: Option<u64>,
    /// 是否当前选中（菜单勾选态）
    pub selected: bool,
}

fn delay_color(ms: Option<u64>, badge: &str) -> Retained<NSColor> {
    if badge == "失败" {
        return NSColor::systemRedColor();
    }
    match ms {
        Some(ms) if ms <= 300 => NSColor::systemGreenColor(),
        Some(_) => NSColor::systemOrangeColor(),
        _ => NSColor::systemGrayColor(),
    }
}

// 生成带圆角底色的原生菜单徽标。使用附件图像而非 NSBackgroundColorAttributeName，
// 后者只能画方形字符底色，而且菜单重绘时容易被系统样式覆盖。
fn delay_badge_image(badge: &str, color: &NSColor) -> Retained<NSImage> {
    let width = (display_width(badge) as f64 * 5.6 + 11.0).max(28.0);
    let height = 15.0;
    unsafe {
        let image = NSImage::initWithSize(
            NSImage::alloc(),
            objc2_foundation::NSSize::new(width, height),
        );
        image.lockFocus();

        color.setFill();
        let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
            objc2_foundation::NSRect::new(
                objc2_foundation::NSPoint::new(0.0, 0.0),
                objc2_foundation::NSSize::new(width, height),
            ),
            3.5,
            3.5,
        );
        path.fill();

        let font = NSFont::monospacedDigitSystemFontOfSize_weight(9.5, NSFontWeightRegular);
        let white = NSColor::whiteColor();
        let keys: &[&NSString] = &[NSFontAttributeName, NSForegroundColorAttributeName];
        let values: &[&AnyObject] = &[&font, &white];
        let attrs = NSDictionary::from_slices(keys, values);
        NSString::from_str(badge).drawAtPoint_withAttributes(
            objc2_foundation::NSPoint::new(5.5, 2.5),
            Some(&attrs),
        );
        image.unlockFocus();
        image
    }
}

// 按 ClashX 的菜单排版：略小的系统菜单字体显示名称，右侧制表位对齐彩色延迟徽标。
fn node_row_attributed(
    name: &str,
    badge: &str,
    ms: Option<u64>,
    badge_tab: f64,
) -> Retained<NSMutableAttributedString> {
    unsafe {
        let display_name = truncate_display_name(name, 202.0);
        let line = format!("{display_name}\t\u{FFFC}");
        let font = NSFont::menuBarFontOfSize(13.5);
        let para = NSMutableParagraphStyle::new();
        let empty_options = NSDictionary::<objc2_app_kit::NSTextTabOptionKey, AnyObject>::new();
        let tab = objc2_app_kit::NSTextTab::initWithTextAlignment_location_options(
            objc2_app_kit::NSTextTab::alloc(),
            NSTextAlignment::Right,
            badge_tab,
            &empty_options,
        );
        let stops = NSArray::from_slice(&[&*tab]);
        para.setTabStops(Some(&stops));

        let base_keys: &[&NSString] = &[NSFontAttributeName, NSParagraphStyleAttributeName];
        let base_values: &[&AnyObject] = &[&font, &para];
        let base_attrs = NSDictionary::from_slices(base_keys, base_values);
        let main = NSMutableAttributedString::initWithString_attributes(
            NSMutableAttributedString::alloc(),
            &NSString::from_str(&line),
            Some(&base_attrs),
        );

        let color = delay_color(ms, badge);
        let badge_image = delay_badge_image(badge, &color);
        let attachment = NSTextAttachment::new();
        attachment.setImage(Some(&badge_image));
        // NSTextAttachment aligns to the text baseline; lower it slightly so
        // the smaller badge sits centered in the menu row.
        attachment.setBounds(objc2_foundation::NSRect::new(
            objc2_foundation::NSPoint::new(0.0, -1.5),
            objc2_foundation::NSSize::new(
                (display_width(badge) as f64 * 5.6 + 11.0).max(28.0),
                15.0,
            ),
        ));
        let badge_range =
            objc2_foundation::NSRange::new(display_name.encode_utf16().count() + 1, 1);
        let badge_attrs = NSDictionary::from_slices(
            &[NSAttachmentAttributeName],
            &[&*attachment as &AnyObject],
        );
        main.addAttributes_range(&badge_attrs, badge_range);
        main
    }
}

pub fn display_name(name: &str) -> String {
    truncate_display_name(name, 202.0)
}

fn truncate_display_name(name: &str, max_width: f64) -> String {
    let width = |text: &str| display_width(text) as f64 * 7.1;
    if width(name) <= max_width {
        return name.to_string();
    }

    let mut out = String::new();
    for ch in name.chars() {
        let next = format!("{out}{ch}…");
        if width(&next) > max_width {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

// 原地更新托盘菜单的节点行（不重建菜单，菜单打开时可见实时变化）。
pub fn update_node_items(handle: &AppHandle, rows: &[TrayNodeRow], testing: bool) {
    let state = handle.state::<crate::AppState>();
    let tray = state.tray.lock().unwrap().clone();
    let Some(tray) = tray else { return };
    let rows_owned: Vec<TrayNodeRow> = rows.to_vec();
    let testing_owned = testing;
    let _ = tray.with_inner_tray_icon(move |inner| {
        let Some(status_item) = inner.ns_status_item() else { return };
        let Some(mtm) = MainThreadMarker::new() else { return };
        let Some(menu) = status_item.menu(mtm) else { return };
        let (menu_width, badge_tab) = node_menu_layout(&rows_owned);
        update_menu_items(&menu, &rows_owned, testing_owned, menu_width, badge_tab);
    });
}

/// Tauri 的 macOS 托盘菜单存放在状态按钮的 TrayTarget 子视图中；菜单项动作返回后，
/// NSStatusItem.menu 已经被清空。向该子视图发送一次合成点击，会沿用 Tauri 保存的
/// 同一个菜单并进入原生菜单跟踪循环，让测速期间菜单保持可见，而不是调用失效的 menu 属性。
pub fn keep_menu_open(handle: &AppHandle) {
    let Some(state) = handle.try_state::<crate::AppState>() else { return };
    let tray = state.tray.lock().unwrap().clone();
    let Some(tray) = tray else { return };
    let _ = tray.with_inner_tray_icon(move |inner| {
        let Some(status_item) = inner.ns_status_item() else { return };
        let Some(mtm) = MainThreadMarker::new() else { return };
        let Some(button) = status_item.button(mtm) else { return };
        let Some(target) = button.subviews().firstObject() else { return };
        let Some(window) = target.window() else { return };
        let location = button.convertPoint_toView(
            objc2_foundation::NSPoint::new(1.0, 1.0),
            None,
        );
        let Some(down) = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            NSEventType::LeftMouseDown,
            location,
            NSEventModifierFlags::empty(),
            0.0,
            window.windowNumber(),
            None,
            0,
            1,
            1.0,
        ) else { return };
        let Some(up) = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            NSEventType::LeftMouseUp,
            location,
            NSEventModifierFlags::empty(),
            0.0,
            window.windowNumber(),
            None,
            0,
            1,
            0.0,
        ) else { return };

        unsafe {
            let _: () = objc2::msg_send![&*target, mouseDown: &*down];
            let _: () = objc2::msg_send![&*target, mouseUp: &*up];
        }
    });
}

fn node_menu_layout(_rows: &[TrayNodeRow]) -> (f64, f64) {
    // Fixed menu width and right edge for the badge column. Long node names are
    // truncated to keep this width stable after every benchmark result.
    (340.0, 310.0)
}

fn update_menu_items(
    menu: &NSMenu,
    rows: &[TrayNodeRow],
    testing: bool,
    menu_width: f64,
    badge_tab: f64,
) {
    let is_node_menu = (0..menu.numberOfItems()).any(|index| {
        menu.itemAtIndex(index).is_some_and(|item| {
            matches!(item.title().to_string().as_str(), "延迟测速" | "延迟测速中…" | "Test latency" | "Testing latency…")
        })
    });
    if is_node_menu {
        menu.setMinimumWidth(menu_width);
    }

    let mut row_index = 0;
    for index in 0..menu.numberOfItems() {
        let Some(item) = menu.itemAtIndex(index) else { continue };
        let title = item.title().to_string();
        if matches!(
            title.as_str(),
            "延迟测速" | "延迟测速中…" | "Test latency" | "Testing latency…"
        ) {
            let chinese = title.starts_with("延迟");
            let text = match (testing, chinese) {
                (true, true) => "延迟测速中…",
                (false, true) => "延迟测速",
                (true, false) => "Testing latency…",
                (false, false) => "Test latency",
            };
            item.setTitle(&NSString::from_str(text));
            item.setEnabled(!testing);
        } else if is_node_menu && !title.is_empty() {
            let represented_name = item
                .representedObject()
                .and_then(|value| value.downcast::<NSString>().ok())
                .map(|value| value.to_string());
            let row = represented_name
                .as_deref()
                .and_then(|name| rows.iter().find(|row| row.name == name))
                .or_else(|| rows.get(row_index));
            row_index += 1;
            if let Some(row) = row {
                unsafe {
                    item.setRepresentedObject(Some(&NSString::from_str(&row.name)));
                }
                let attr = node_row_attributed(&row.name, &row.badge, row.ms, badge_tab);
                item.setAttributedTitle(Some(&attr));
                item.setState(if row.selected { 1 } else { 0 });
            }
        }

        if let Some(submenu) = item.submenu() {
            update_menu_items(&submenu, rows, testing, menu_width, badge_tab);
        }
    }
}
