//! macOS 菜单栏实时速率：NSStatusItem 富文本双行标题（借鉴 Clash Verge Rev 的
//! 公开技术方案，实现独立编写）。
//!
//! 系统菜单栏高 24pt，默认字号塞不下两行；方案：9pt 等宽字体 + 固定 10pt 行高 +
//! 精确基线偏移把两行垂直居中在按钮内，并按内容宽度设定 status item 长度以避免
//! 每秒抖动。仅 macOS，其余平台为空操作。

use crate::tray::helper::{self, Command, Event};
use crate::tray::menu::Entry;
use crate::tray::state::{TrayMenuData, TrayNodeRow};
use std::cell::{Cell, RefCell};
use tauri::{AppHandle, Manager};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AnyThread, DeclaredClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::NSAppearanceCustomization as _;
use objc2_app_kit::NSAttributedStringNSStringDrawing as _;
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameDarkAqua, NSApplication, NSBaselineOffsetAttributeName,
    NSCellImagePosition, NSColor, NSControlSize, NSEvent, NSFont, NSFontAttributeName,
    NSFontWeightMedium, NSFontWeightRegular, NSForegroundColorAttributeName, NSImage, NSImageView,
    NSLineBreakMode, NSMenu, NSMenuDelegate, NSMenuItem, NSMutableParagraphStyle,
    NSParagraphStyleAttributeName, NSProgressIndicator, NSProgressIndicatorStyle, NSStatusBar,
    NSStatusItem, NSTextAlignment, NSTextField, NSView,
};
use objc2_foundation::{
    NSAttributedString, NSAttributedStringKey, NSData, NSDictionary, NSNumber, NSObject,
    NSObjectProtocol, NSString,
};

const FONT_SIZE: f64 = 9.5;
const LINE_HEIGHT: f64 = 10.0;
// 图标占位 + 系统内边距（Tauri 图标 18pt）。
const EXTRA_WIDTH: f64 = 30.0;
// 短值（如 ↓0B↑0B）防抖的最小宽度。
const MIN_LENGTH: f64 = 56.0;
const NS_VARIABLE_LENGTH: f64 = -1.0;

thread_local! {
    static STATUS_MENU: std::cell::OnceCell<Retained<StatusMenu>> = const { std::cell::OnceCell::new() };
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

/// NSTextField label cells in custom NSMenuItem views align their baseline too
/// high in these compact rows. Shift glyphs down without moving the row or its
/// background (notably the latency badge) so all text is optically centered.
fn set_menu_label_text(label: &NSTextField, text: &str) {
    set_menu_label_text_with_offset(label, text, -2.0);
}

fn set_menu_label_text_with_offset(label: &NSTextField, text: &str, baseline_offset: f64) {
    unsafe {
        let value = NSString::from_str(text);
        let offset = NSNumber::new_f64(baseline_offset);
        let attrs =
            NSDictionary::from_slices(&[NSBaselineOffsetAttributeName], &[&*offset as &AnyObject]);
        let attributed = NSAttributedString::new_with_attributes(&value, &attrs);
        label.setAttributedStringValue(&attributed);
    }
}

/// 徽标文字：居中段落样式 + 基线下移；返回**实测**文本宽度。
/// 徽标底色按实测宽度绘制——估算公式在 timeout/failed 等长文本上
/// 会留出大块右侧空白，且 attributed 文本不受字段对齐属性控制，
/// 必须在段落样式里显式居中。
fn set_badge_text(badge: &NSTextField, text: &str) -> f64 {
    unsafe {
        let font = NSFont::monospacedDigitSystemFontOfSize_weight(9.0, NSFontWeightRegular);
        let color = NSColor::whiteColor();
        let value = NSString::from_str(text);
        let offset = NSNumber::new_f64(-2.0);
        let para = NSMutableParagraphStyle::new();
        para.setAlignment(NSTextAlignment::Center);
        let keys: &[&NSAttributedStringKey] = &[
            NSFontAttributeName,
            NSForegroundColorAttributeName,
            NSParagraphStyleAttributeName,
            NSBaselineOffsetAttributeName,
        ];
        let values: &[&AnyObject] = &[&font, &color, &para, &offset];
        let attrs = NSDictionary::from_slices(keys, values);
        use objc2_app_kit::NSAttributedStringNSStringDrawing as _;
        let attributed = NSAttributedString::new_with_attributes(&value, &attrs);
        let width = attributed.size().width;
        badge.setAttributedStringValue(&attributed);
        width
    }
}

// AppKit owns the button hit area, including the attributed speed title.
fn apply(status_item: &NSStatusItem, text: &str, show_speed: bool) {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("tray_speed: not on main thread, skip");
        return;
    };
    let Some(button) = status_item.button(mtm) else {
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
    send(handle, Command::Speed(speed_text(up, down)));
}

pub fn clear(handle: &AppHandle) {
    send(handle, Command::Speed(String::new()));
}

fn with_status_menu<T>(f: impl FnOnce(&StatusMenu) -> T) -> Option<T> {
    STATUS_MENU.with(|slot| slot.get().map(|tray| f(tray)))
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
fn delay_color(ms: Option<u64>, badge: &str) -> Retained<NSColor> {
    // 失败徽标原因（timeout / failed）——红色；旧的「失败」文案已废弃
    if badge == "timeout" || badge == "failed" {
        return NSColor::systemRedColor();
    }
    match ms {
        Some(ms) if ms <= 300 => NSColor::systemGreenColor(),
        Some(_) => NSColor::systemOrangeColor(),
        _ => NSColor::systemGrayColor(),
    }
}

const ROW_VIEW_NAME_TAG: isize = 101;
const ROW_VIEW_CHECK_TAG: isize = 102;
const ROW_VIEW_BADGE_TAG: isize = 103;
const ROW_VIEW_CURRENT_TAG: isize = 107;
const LATENCY_COLUMN_WIDTH: f64 = 50.0;
const CHECK_ROW_VIEW_CHECK_TAG: isize = 108;
const CHECK_ROW_VIEW_LABEL_TAG: isize = 109;
const ACTION_ROW_VIEW_LABEL_TAG: isize = 104;
const NODE_MENU_WIDTH: f64 = 270.0;
const NODE_ROW_HEIGHT: f64 = 23.0;
const ACTION_ROW_HEIGHT: f64 = NODE_ROW_HEIGHT;
const HOVER_HORIZONTAL_INSET: f64 = 6.0;
const HOVER_VERTICAL_INSET: f64 = 1.0;
const ROW_CONTENT_INSET: f64 = 6.0;
const HOVER_CORNER_RADIUS: f64 = 7.0;

// NSMenuItem custom views do not automatically dispatch the menu item's action.
// Forward the click to the native target so Crossbow still receives the
// same action as it does for a standard check-menu item.
define_class!(
    #[unsafe(super(NSView))]
    #[name = "CrossbowTrayNodeRowView"]
    #[thread_kind = MainThreadOnly]
    struct TrayNodeRowView;

    unsafe impl NSObjectProtocol for TrayNodeRowView {}

    impl TrayNodeRowView {
        #[unsafe(method(viewDidMoveToWindow))]
        fn view_did_move_to_window(&self) {
            unsafe { let _: () = msg_send![super(self), viewDidMoveToWindow]; }
            let pending = self.window().is_some() && self.viewWithTag(ROW_VIEW_BADGE_TAG)
                .is_some_and(|badge| badge.isHidden());
            set_node_spinner_running(self, pending);
        }

        #[unsafe(method(mouseDown:))]
        fn on_mouse_down(&self, _event: &NSEvent) {
            let Some(item) = self.enclosingMenuItem() else { return };
            if let Some(action) = item.action() {
                let mtm = MainThreadMarker::new().expect("menu click runs on the main thread");
                let app = NSApplication::sharedApplication(mtm);
                let target = item.target();
                unsafe {
                    app.sendAction_to_from(action, target.as_deref(), Some(&*item));
                }
            }
            // Keep the full status-item menu open; selection is applied and
            // the checked row is refreshed in place by the async handler.
        }
    }
);

// Unlike node selection, triggering latency testing should leave the whole
// menu hierarchy open. A custom action row forwards the action itself without
// cancelling NSMenu tracking, allowing the same submenu to refresh in place.
define_class!(
    #[unsafe(super(NSView))]
    #[name = "CrossbowTrayActionRowView"]
    #[thread_kind = MainThreadOnly]
    struct TrayActionRowView;

    unsafe impl NSObjectProtocol for TrayActionRowView {}

    impl TrayActionRowView {
        #[unsafe(method(mouseDown:))]
        fn on_mouse_down(&self, _event: &NSEvent) {
            let Some(item) = self.enclosingMenuItem() else { return };
            if let Some(action) = item.action() {
                let mtm = MainThreadMarker::new().expect("menu click runs on the main thread");
                let app = NSApplication::sharedApplication(mtm);
                let target = item.target();
                unsafe {
                    app.sendAction_to_from(action, target.as_deref(), Some(&*item));
                }
            }
        }
    }
);

// Mode and profile entries use the same bold check and compact hover row as
// the selected node. Forward their native action, then close the submenu.
define_class!(
    #[unsafe(super(NSView))]
    #[name = "CrossbowTrayCheckRowView"]
    #[thread_kind = MainThreadOnly]
    struct TrayCheckRowView;

    unsafe impl NSObjectProtocol for TrayCheckRowView {}

    impl TrayCheckRowView {
        #[unsafe(method(mouseDown:))]
        fn on_mouse_down(&self, _event: &NSEvent) {
            let Some(item) = self.enclosingMenuItem() else { return };
            let menu = unsafe { item.menu() };
            if let Some(action) = item.action() {
                let mtm = MainThreadMarker::new().expect("menu click runs on the main thread");
                let app = NSApplication::sharedApplication(mtm);
                let target = item.target();
                unsafe {
                    app.sendAction_to_from(action, target.as_deref(), Some(&*item));
                }
            }
            if let Some(menu) = menu {
                menu.cancelTracking();
            }
        }
    }
);

fn make_check_row_view(mtm: MainThreadMarker, width: f64) -> Retained<TrayCheckRowView> {
    let frame = objc2_foundation::NSRect::new(
        objc2_foundation::NSPoint::new(0.0, 0.0),
        objc2_foundation::NSSize::new(width, NODE_ROW_HEIGHT),
    );
    let view: Retained<TrayCheckRowView> = unsafe {
        let allocated = TrayCheckRowView::alloc(mtm);
        msg_send![allocated, initWithFrame: frame]
    };
    view.setWantsLayer(true);
    add_hover_background(&view, width, NODE_ROW_HEIGHT, mtm);

    let check = NSTextField::labelWithString(&NSString::from_str("✓"), mtm);
    check.setTag(CHECK_ROW_VIEW_CHECK_TAG);
    check.setFont(Some(&NSFont::boldSystemFontOfSize(14.0)));
    check.setAlignment(NSTextAlignment::Center);
    check.setFrame(objc2_foundation::NSRect::new(
        objc2_foundation::NSPoint::new(HOVER_HORIZONTAL_INSET + 1.0, 0.0),
        objc2_foundation::NSSize::new(18.0, NODE_ROW_HEIGHT),
    ));
    let label = NSTextField::labelWithString(&NSString::new(), mtm);
    label.setTag(CHECK_ROW_VIEW_LABEL_TAG);
    label.setFont(Some(&NSFont::menuBarFontOfSize(12.5)));
    label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    label.setFrame(objc2_foundation::NSRect::new(
        objc2_foundation::NSPoint::new(26.0, 0.0),
        objc2_foundation::NSSize::new(width - 38.0, NODE_ROW_HEIGHT),
    ));
    view.addSubview(&check);
    view.addSubview(&label);
    view
}

fn update_check_row_view(view: &TrayCheckRowView, title: &str, checked: bool, width: f64) {
    view.setFrame(objc2_foundation::NSRect::new(
        objc2_foundation::NSPoint::new(0.0, 0.0),
        objc2_foundation::NSSize::new(width, NODE_ROW_HEIGHT),
    ));
    resize_hover_background(view, width, NODE_ROW_HEIGHT);
    if let Some(check) = view
        .viewWithTag(CHECK_ROW_VIEW_CHECK_TAG)
        .and_then(|view| view.downcast::<NSTextField>().ok())
    {
        check.setHidden(!checked);
    }
    if let Some(label) = view
        .viewWithTag(CHECK_ROW_VIEW_LABEL_TAG)
        .and_then(|view| view.downcast::<NSTextField>().ok())
    {
        set_menu_label_text(&label, title);
        label.setToolTip(Some(&NSString::from_str(title)));
        label.setFrame(objc2_foundation::NSRect::new(
            objc2_foundation::NSPoint::new(26.0, 0.0),
            objc2_foundation::NSSize::new(width - 38.0, NODE_ROW_HEIGHT),
        ));
    }
}

fn make_action_row_view(mtm: MainThreadMarker) -> Retained<TrayActionRowView> {
    let frame = objc2_foundation::NSRect::new(
        objc2_foundation::NSPoint::new(0.0, 0.0),
        objc2_foundation::NSSize::new(NODE_MENU_WIDTH, ACTION_ROW_HEIGHT),
    );
    let view: Retained<TrayActionRowView> = unsafe {
        let allocated = TrayActionRowView::alloc(mtm);
        msg_send![allocated, initWithFrame: frame]
    };
    view.setWantsLayer(true);
    add_hover_background(&view, NODE_MENU_WIDTH, ACTION_ROW_HEIGHT, mtm);
    let label = NSTextField::labelWithString(&NSString::new(), mtm);
    label.setTag(ACTION_ROW_VIEW_LABEL_TAG);
    label.setFrame(objc2_foundation::NSRect::new(
        objc2_foundation::NSPoint::new(18.0, 0.0),
        objc2_foundation::NSSize::new(NODE_MENU_WIDTH - 36.0, ACTION_ROW_HEIGHT),
    ));
    label.setFont(Some(&NSFont::menuBarFontOfSize(13.0)));
    view.addSubview(&label);
    view
}

fn update_action_row_view(view: &TrayActionRowView, text: &str, enabled: bool, menu_width: f64) {
    let Some(label) = view
        .viewWithTag(ACTION_ROW_VIEW_LABEL_TAG)
        .and_then(|view| view.downcast::<NSTextField>().ok())
    else {
        return;
    };
    set_menu_label_text(&label, text);
    let color = if enabled {
        NSColor::labelColor()
    } else {
        NSColor::disabledControlTextColor()
    };
    label.setTextColor(Some(&color));
    label.setFrame(objc2_foundation::NSRect::new(
        objc2_foundation::NSPoint::new(18.0, 0.0),
        objc2_foundation::NSSize::new((menu_width - 36.0).max(100.0), ACTION_ROW_HEIGHT),
    ));
    resize_hover_background(view, menu_width, ACTION_ROW_HEIGHT);
}

fn make_node_row_view(mtm: MainThreadMarker) -> Retained<TrayNodeRowView> {
    let frame = objc2_foundation::NSRect::new(
        objc2_foundation::NSPoint::new(0.0, 0.0),
        objc2_foundation::NSSize::new(NODE_MENU_WIDTH, NODE_ROW_HEIGHT),
    );
    let view: Retained<TrayNodeRowView> = unsafe {
        let allocated = TrayNodeRowView::alloc(mtm);
        msg_send![allocated, initWithFrame: frame]
    };
    view.setWantsLayer(true);
    add_hover_background(&view, NODE_MENU_WIDTH, NODE_ROW_HEIGHT, mtm);

    let check = NSTextField::labelWithString(&NSString::from_str("✓"), mtm);
    check.setTag(ROW_VIEW_CHECK_TAG);
    check.setFont(Some(&NSFont::boldSystemFontOfSize(14.0)));
    check.setAlignment(NSTextAlignment::Center);
    check.setFrame(objc2_foundation::NSRect::new(
        objc2_foundation::NSPoint::new(HOVER_HORIZONTAL_INSET + 1.0, 0.0),
        objc2_foundation::NSSize::new(18.0, NODE_ROW_HEIGHT),
    ));
    check.setHidden(true);

    let name = NSTextField::labelWithString(&NSString::new(), mtm);
    name.setTag(ROW_VIEW_NAME_TAG);
    name.setFrame(objc2_foundation::NSRect::new(
        objc2_foundation::NSPoint::new(26.0, 0.0),
        objc2_foundation::NSSize::new(220.0, NODE_ROW_HEIGHT),
    ));
    name.setFont(Some(&NSFont::menuBarFontOfSize(12.5)));
    name.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);

    let current = NSTextField::labelWithString(&NSString::new(), mtm);
    current.setTag(ROW_VIEW_CURRENT_TAG);
    current.setFont(Some(&NSFont::menuBarFontOfSize(9.5)));
    current.setAlignment(NSTextAlignment::Center);
    current.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    current.setTextColor(Some(&NSColor::secondaryLabelColor()));
    current.setHidden(true);

    let badge = NSTextField::labelWithString(&NSString::new(), mtm);
    badge.setTag(ROW_VIEW_BADGE_TAG);
    badge.setAlignment(NSTextAlignment::Center);
    let badge_font =
        unsafe { NSFont::monospacedDigitSystemFontOfSize_weight(9.0, NSFontWeightRegular) };
    badge.setFont(Some(&badge_font));
    badge.setTextColor(Some(&NSColor::whiteColor()));
    badge.setWantsLayer(true);
    if let Some(layer) = layer_for(&badge) {
        unsafe {
            let _: () = msg_send![&*layer, setCornerRadius: 3.0f64];
            let _: () = msg_send![&*layer, setMasksToBounds: true];
        }
    }

    let spinner: Retained<NSProgressIndicator> =
        unsafe { msg_send![NSProgressIndicator::alloc(mtm), initWithFrame: frame] };
    spinner.setStyle(NSProgressIndicatorStyle::Spinning);
    spinner.setControlSize(NSControlSize::Small);
    spinner.setIndeterminate(true);
    spinner.setDisplayedWhenStopped(false);
    // Menu tracking uses a separate run-loop mode; let AppKit animate independently.
    unsafe {
        spinner.setUsesThreadedAnimation(true);
    }
    spinner.setHidden(true);

    view.addSubview(&check);
    view.addSubview(&name);
    view.addSubview(&current);
    view.addSubview(&badge);
    view.addSubview(&spinner);
    view
}

fn update_node_row_view(
    view: &TrayNodeRowView,
    row: &TrayNodeRow,
    ms: Option<u64>,
    menu_width: f64,
    mtm: MainThreadMarker,
) {
    let Some(name) = view
        .viewWithTag(ROW_VIEW_NAME_TAG)
        .and_then(|view| view.downcast::<NSTextField>().ok())
    else {
        return;
    };
    let Some(check) = view
        .viewWithTag(ROW_VIEW_CHECK_TAG)
        .and_then(|view| view.downcast::<NSTextField>().ok())
    else {
        return;
    };
    let Some(badge) = view
        .viewWithTag(ROW_VIEW_BADGE_TAG)
        .and_then(|view| view.downcast::<NSTextField>().ok())
    else {
        return;
    };
    let Some(current) = view
        .viewWithTag(ROW_VIEW_CURRENT_TAG)
        .and_then(|view| view.downcast::<NSTextField>().ok())
    else {
        return;
    };

    let Some(spinner) = node_spinner(view) else {
        return;
    };
    let badge_text_width = set_badge_text(&badge, &row.badge);
    let badge_width = (badge_text_width + 9.0).clamp(26.0, LATENCY_COLUMN_WIDTH);
    let column_right = menu_width - HOVER_HORIZONTAL_INSET - ROW_CONTENT_INSET;
    let column_left = column_right - LATENCY_COLUMN_WIDTH;
    let badge_x = column_right - badge_width;
    let current_text = row
        .current_node
        .as_ref()
        .map(|node| format_flag_spacing(node));
    let name_width = if current_text.is_some() {
        (display_width(&row.label) as f64 * 5.8 + 4.0).clamp(66.0, 88.0)
    } else {
        (column_left - 38.0).max(50.0)
    };
    let spaced_name = format_flag_spacing(&row.label);
    let display_name = truncate_display_name(&spaced_name, name_width);
    set_menu_label_text(&name, &display_name);
    name.setToolTip(Some(&NSString::from_str(&spaced_name)));
    check.setHidden(!row.selected);
    name.setFrame(objc2_foundation::NSRect::new(
        objc2_foundation::NSPoint::new(26.0, 0.0),
        objc2_foundation::NSSize::new(name_width, NODE_ROW_HEIGHT),
    ));
    if let Some(current_text) = current_text {
        let current_x = 26.0 + name_width + 4.0;
        let current_width = (column_left - current_x - 5.0).max(0.0);
        set_menu_label_text_with_offset(
            &current,
            &truncate_display_name_with_unit(&current_text, current_width, 5.2),
            -4.5,
        );
        current.setToolTip(Some(&NSString::from_str(&format!(
            "{}: {}",
            row.current_label, current_text
        ))));
        current.setFrame(objc2_foundation::NSRect::new(
            objc2_foundation::NSPoint::new(current_x, 0.0),
            objc2_foundation::NSSize::new(current_width, NODE_ROW_HEIGHT),
        ));
        current.setHidden(current_width < 28.0);
    } else {
        current.setHidden(true);
    }
    badge.setFrame(objc2_foundation::NSRect::new(
        objc2_foundation::NSPoint::new(badge_x, 3.5),
        objc2_foundation::NSSize::new(badge_width, 16.0),
    ));
    badge.setHidden(row.pending);
    // Keep a fixed latency column so switching from spinner to badge never shifts labels.
    spinner.setFrame(objc2_foundation::NSRect::new(
        objc2_foundation::NSPoint::new(column_right - 14.0, (NODE_ROW_HEIGHT - 14.0) / 2.0),
        objc2_foundation::NSSize::new(14.0, 14.0),
    ));
    let testing_label = NSString::from_str(if row.current_label == "当前" {
        "测速中…"
    } else {
        "Testing…"
    });
    spinner.setToolTip(Some(&testing_label));
    unsafe {
        let _: () = msg_send![&*spinner, setAccessibilityLabel: &*testing_label];
    }
    set_node_spinner_running(view, row.pending && view.window().is_some());
    let color = delay_color(ms, &row.badge);
    if let Some(layer) = layer_for(&badge) {
        let cg_color = color.CGColor();
        unsafe {
            let _: () = msg_send![&*layer, setBackgroundColor: &*cg_color];
            let _: () = msg_send![&*layer, setCornerRadius: 3.0f64];
            let _: () = msg_send![&*layer, setMasksToBounds: true];
        }
    }
    let _ = mtm;
}

// The spinner is the last child created by make_node_row_view (like the first
// child reserved for the hover background); NSProgressIndicator has no setTag.
fn node_spinner(view: &NSView) -> Option<Retained<NSProgressIndicator>> {
    view.subviews()
        .lastObject()?
        .downcast::<NSProgressIndicator>()
        .ok()
}

fn set_node_spinner_running(view: &NSView, running: bool) {
    let Some(spinner) = node_spinner(view) else {
        return;
    };
    unsafe {
        if running {
            spinner.setHidden(false);
            spinner.startAnimation(None);
        } else {
            spinner.stopAnimation(None);
            spinner.setHidden(true);
        }
    }
}

fn add_hover_background(view: &NSView, width: f64, height: f64, mtm: MainThreadMarker) {
    let frame = objc2_foundation::NSRect::new(
        objc2_foundation::NSPoint::new(HOVER_HORIZONTAL_INSET, HOVER_VERTICAL_INSET),
        objc2_foundation::NSSize::new(
            (width - HOVER_HORIZONTAL_INSET * 2.0).max(0.0),
            (height - HOVER_VERTICAL_INSET * 2.0).max(0.0),
        ),
    );
    let background: Retained<NSView> = unsafe {
        let allocated = NSView::alloc(mtm);
        msg_send![allocated, initWithFrame: frame]
    };
    background.setWantsLayer(true);
    view.addSubview(&background);
}

fn hover_background(view: &NSView) -> Option<Retained<NSView>> {
    view.subviews().firstObject()?.downcast::<NSView>().ok()
}

fn resize_hover_background(view: &NSView, width: f64, height: f64) {
    if let Some(background) = hover_background(view) {
        background.setFrame(objc2_foundation::NSRect::new(
            objc2_foundation::NSPoint::new(HOVER_HORIZONTAL_INSET, HOVER_VERTICAL_INSET),
            objc2_foundation::NSSize::new(
                (width - HOVER_HORIZONTAL_INSET * 2.0).max(0.0),
                (height - HOVER_VERTICAL_INSET * 2.0).max(0.0),
            ),
        ));
    }
}

fn set_row_hovered(view: &NSView, hovered: bool, label_tags: &[isize]) {
    let Some(background_view) = hover_background(view) else {
        return;
    };
    let Some(layer) = layer_for(&background_view) else {
        return;
    };
    let background = if hovered {
        NSColor::selectedContentBackgroundColor()
    } else {
        NSColor::clearColor()
    };
    let cg_color = background.CGColor();
    unsafe {
        let _: () = msg_send![&*layer, setBackgroundColor: &*cg_color];
        let _: () = msg_send![&*layer, setCornerRadius: HOVER_CORNER_RADIUS];
        let _: () = msg_send![&*layer, setMasksToBounds: true];
    }
    let text_color = if hovered {
        NSColor::whiteColor()
    } else {
        NSColor::labelColor()
    };
    if let Some(spinner) = node_spinner(view) {
        // Custom menu highlights do not automatically switch a child's appearance.
        // Dark Aqua gives the native spinner a light glyph on the blue hover row.
        let appearance = if hovered {
            unsafe { NSAppearance::appearanceNamed(NSAppearanceNameDarkAqua) }
        } else {
            None
        };
        spinner.setAppearance(appearance.as_deref());
    }
    for tag in label_tags {
        if let Some(label) = view.viewWithTag(*tag) {
            if let Ok(label) = label.clone().downcast::<NSTextField>() {
                label.setTextColor(Some(&text_color));
            } else if let Ok(icon) = label.downcast::<NSImageView>() {
                icon.setContentTintColor(Some(&text_color));
            }
        }
    }
}

fn set_action_row_hovered(view: &TrayActionRowView, hovered: bool) {
    let enabled = view
        .enclosingMenuItem()
        .is_some_and(|item| item.isEnabled());
    set_row_hovered(view, hovered && enabled, &[ACTION_ROW_VIEW_LABEL_TAG]);
    if !enabled {
        if let Some(label) = view
            .viewWithTag(ACTION_ROW_VIEW_LABEL_TAG)
            .and_then(|view| view.downcast::<NSTextField>().ok())
        {
            label.setTextColor(Some(&NSColor::disabledControlTextColor()));
        }
    }
}

fn set_menu_item_hovered(item: &NSMenuItem, hovered: bool) {
    let Some(view) = item.view() else { return };
    if let Ok(view) = view.clone().downcast::<TrayNodeRowView>() {
        set_row_hovered(
            &view,
            hovered,
            &[ROW_VIEW_NAME_TAG, ROW_VIEW_CURRENT_TAG, ROW_VIEW_CHECK_TAG],
        );
    } else if let Ok(view) = view.clone().downcast::<TrayActionRowView>() {
        set_action_row_hovered(&view, hovered);
    } else if let Ok(view) = view.downcast::<TrayCheckRowView>() {
        set_row_hovered(
            &view,
            hovered,
            &[CHECK_ROW_VIEW_CHECK_TAG, CHECK_ROW_VIEW_LABEL_TAG],
        );
    }
}

fn layer_for(view: &NSView) -> Option<Retained<AnyObject>> {
    unsafe { msg_send![view, layer] }
}

struct StatusMenuState {
    dispatch: Box<dyn Fn(Event)>,
    item: Retained<NSStatusItem>,
    tracking: Cell<bool>,
    pending_menu: RefCell<Option<(Vec<Entry>, TrayMenuData)>>,
    speed: RefCell<String>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "CrossbowStatusMenu"]
    #[thread_kind = MainThreadOnly]
    #[ivars = StatusMenuState]
    struct StatusMenu;

    unsafe impl NSObjectProtocol for StatusMenu {}

    unsafe impl NSMenuDelegate for StatusMenu {
        #[unsafe(method(menuWillOpen:))]
        fn menu_will_open(&self, menu: &NSMenu) {
            if self.is_root(menu) {
                self.ivars().tracking.set(true);
            }
            if is_node_menu(menu) {
                (self.ivars().dispatch)(Event::RefreshNodes);
            }
            for item in menu.itemArray().iter() {
                if let Some(view) = item.view() {
                    let pending = view.viewWithTag(ROW_VIEW_BADGE_TAG).is_some_and(|badge| badge.isHidden());
                    set_node_spinner_running(&view, pending);
                }
            }
        }

        #[unsafe(method(menuDidClose:))]
        fn menu_did_close(&self, menu: &NSMenu) {
            for item in menu.itemArray().iter() {
                if let Some(view) = item.view() {
                    set_node_spinner_running(&view, false);
                }
                set_menu_item_hovered(&item, false);
            }
            if self.is_root(menu) {
                self.ivars().tracking.set(false);
                // Replace the attached menu after AppKit finishes the close.
                dispatch2::DispatchQueue::main().exec_async(|| {
                    with_status_menu(|tray| {
                        let pending = tray.ivars().pending_menu.take();
                        if let Some((entries, data)) = pending {
                            tray.rebuild(entries, data);
                        }
                    });
                });
            }
        }

        #[unsafe(method(menu:willHighlightItem:))]
        #[allow(non_snake_case)]
        fn menu_willHighlightItem(&self, menu: &NSMenu, highlighted: Option<&NSMenuItem>) {
            for item in menu.itemArray().iter() {
                let is_highlighted = highlighted.is_some_and(|candidate| {
                    std::ptr::eq(&*item, candidate)
                });
                set_menu_item_hovered(&item, is_highlighted);
            }
        }
    }

    impl StatusMenu {
        #[unsafe(method(performMenuAction:))]
        fn perform_menu_action(&self, item: &NSMenuItem) {
            if item.isEnabled() {
                if let Some(id) = item_id(item) {
                    (self.ivars().dispatch)(Event::Action(id));
                }
            }
        }
    }
);

impl StatusMenu {
    fn is_root(&self, menu: &NSMenu) -> bool {
        self.ivars()
            .item
            .menu(MainThreadMarker::from(self))
            .is_some_and(|root| std::ptr::eq(&*root, menu))
    }

    fn rebuild(&self, entries: Vec<Entry>, data: TrayMenuData) {
        if self.ivars().tracking.get() {
            self.ivars().pending_menu.replace(Some((entries, data)));
            return;
        }
        self.ivars().pending_menu.take();
        let root = build_native_menu("", &entries, self);
        update_node_rows(&root, &data.title, &data.rows, data.testing);
        // Leave the menu attached even while closed. AppKit can track it from
        // another Space without an overlay view or a synthetic button click.
        self.ivars().item.setMenu(Some(&root));
    }
}

fn item_id(item: &NSMenuItem) -> Option<String> {
    item.representedObject()?
        .downcast::<NSString>()
        .ok()
        .map(|id| id.to_string())
}

fn is_node_menu(menu: &NSMenu) -> bool {
    menu.itemArray()
        .firstObject()
        .is_some_and(|item| item_id(&item).as_deref() == Some("tray-delay-test"))
}

fn build_native_menu(title: &str, entries: &[Entry], target: &StatusMenu) -> Retained<NSMenu> {
    let mtm = MainThreadMarker::from(target);
    let menu = NSMenu::new(mtm);
    menu.setTitle(&NSString::from_str(title));
    menu.setAutoenablesItems(false);
    menu.setDelegate(Some(ProtocolObject::from_ref(target)));
    let check_width = entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Action { id, title, .. }
                if id.starts_with("mode-") || id.starts_with("profile-") =>
            {
                Some(display_width(title) as f64 * 7.1 + 42.0)
            }
            _ => None,
        })
        .reduce(f64::max)
        .map(|width| width.clamp(110.0, 300.0));
    if let Some(width) = check_width {
        menu.setMinimumWidth(width);
    }
    for entry in entries {
        let item = match entry {
            Entry::Separator => NSMenuItem::separatorItem(mtm),
            Entry::Submenu(title, children) => {
                let item = NSMenuItem::new(mtm);
                item.setTitle(&NSString::from_str(title));
                item.setSubmenu(Some(&build_native_menu(title, children, target)));
                item.setEnabled(true);
                item
            }
            Entry::Action {
                id,
                title,
                enabled,
                checked,
            } => {
                let item = unsafe {
                    let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(mtm),
                        &NSString::from_str(title),
                        Some(objc2::sel!(performMenuAction:)),
                        &NSString::from_str(""),
                    );
                    item.setTarget(Some(target));
                    item.setRepresentedObject(Some(&NSString::from_str(id)));
                    item
                };
                item.setEnabled(*enabled);
                if let Some(checked) = checked {
                    item.setState(isize::from(*checked));
                    if let Some(width) = check_width {
                        let view = make_check_row_view(mtm, width);
                        update_check_row_view(&view, title, *checked, width);
                        item.setView(Some(&view));
                    }
                }
                item
            }
        };
        menu.addItem(&item);
    }
    menu
}

pub fn install(handle: &AppHandle) -> std::io::Result<()> {
    match helper::start(handle) {
        Ok(Some(helper)) => {
            handle.manage(helper);
        }
        Ok(None) => install_in_process(handle)?,
        Err(error) => {
            eprintln!("could not start native tray process: {error}");
            install_in_process(handle)?;
        }
    }
    refresh_menu(handle);
    Ok(())
}

fn install_in_process(handle: &AppHandle) -> std::io::Result<()> {
    let handle = handle.clone();
    install_native(move |event| helper::dispatch(&handle, event))
}

pub(crate) fn helper_stopped(handle: &AppHandle) {
    if let Err(error) = install_in_process(handle) {
        eprintln!("could not restore native status item: {error}");
        return;
    }
    refresh_menu(handle);
    crate::sync_tray_traffic(handle);
}

pub(crate) fn install_native(dispatch: impl Fn(Event) + 'static) -> std::io::Result<()> {
    let mtm = MainThreadMarker::new().expect("tray setup runs on the main thread");
    let image = NSImage::initWithData(
        NSImage::alloc(),
        &NSData::with_bytes(include_bytes!("../../icons/tray-icon.png")),
    )
    .ok_or_else(|| std::io::Error::other("invalid Crossbow status icon"))?;
    image.setTemplate(true);
    image.setSize(objc2_foundation::NSSize::new(18.0, 18.0));
    let bar = NSStatusBar::systemStatusBar();
    let item = bar.statusItemWithLength(NS_VARIABLE_LENGTH);
    let Some(button) = item.button(mtm) else {
        bar.removeStatusItem(&item);
        return Err(std::io::Error::other("missing Crossbow status button"));
    };
    button.setImage(Some(&image));
    button.setImagePosition(NSCellImagePosition::ImageLeft);
    button.setToolTip(Some(&NSString::from_str("Crossbow")));
    let tray = StatusMenu::alloc(mtm).set_ivars(StatusMenuState {
        dispatch: Box::new(dispatch),
        item,
        tracking: Cell::new(false),
        pending_menu: RefCell::new(None),
        speed: RefCell::new(String::new()),
    });
    let tray: Retained<StatusMenu> = unsafe { msg_send![super(tray), init] };
    STATUS_MENU.with(|slot| {
        if let Err(tray) = slot.set(tray) {
            bar.removeStatusItem(&tray.ivars().item);
            return Err(std::io::Error::other(
                "Crossbow status menu already installed",
            ));
        }
        Ok(())
    })?;
    Ok(())
}

pub fn refresh_menu(handle: &AppHandle) {
    let data = crate::tray_menu_data(&handle.state::<crate::AppState>());
    let entries = crate::tray_menu_entries(handle, &data);
    send(handle, Command::Menu(entries, data));
}

fn send(handle: &AppHandle, command: Command) {
    let command = match handle.try_state::<helper::Helper>() {
        Some(helper) => match helper.send(command) {
            Ok(()) => return,
            Err(command) => command,
        },
        None => command,
    };
    let _ = handle.run_on_main_thread(move || apply_command(command));
}

pub(crate) fn apply_command(command: Command) {
    with_status_menu(|tray| match command {
        Command::Menu(entries, data) => tray.rebuild(entries, data),
        Command::Nodes(data) => {
            if !update_status_node_rows(tray, &data.title, &data.rows, data.testing) {
                (tray.ivars().dispatch)(Event::RefreshMenu);
            }
        }
        Command::Speed(text) => {
            let state = tray.ivars();
            if *state.speed.borrow() != text {
                let show = !text.is_empty();
                if !show {
                    state.item.setLength(NS_VARIABLE_LENGTH);
                }
                apply(&state.item, &text, show);
                state.speed.replace(text);
            }
        }
    });
}

pub fn format_flag_spacing(name: &str) -> String {
    let Some((flag, rest)) = name.split_once('|') else {
        return name.to_string();
    };
    let has_flag = flag
        .chars()
        .any(|ch| (0x1F1E6..=0x1F1FF).contains(&(ch as u32)));
    if !has_flag {
        return name.to_string();
    }
    format!("{} | {}", flag.trim_end(), rest.trim_start())
}

fn truncate_display_name(name: &str, max_width: f64) -> String {
    truncate_display_name_with_unit(name, max_width, 7.1)
}

fn truncate_display_name_with_unit(name: &str, max_width: f64, width_per_unit: f64) -> String {
    let width = |text: &str| display_width(text) as f64 * width_per_unit;
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

// Update only a matching menu: when the group changes, old rows must keep
// their own actions until the structural rebuild can safely replace the menu.
pub fn update_node_menu(
    handle: &AppHandle,
    title: &str,
    rows: &[TrayNodeRow],
    testing: bool,
) -> bool {
    if let Some(helper) = handle.try_state::<helper::Helper>() {
        if helper
            .send(Command::Nodes(TrayMenuData {
                title: title.to_owned(),
                rows: rows.to_vec(),
                testing,
            }))
            .is_ok()
        {
            return true;
        }
    }
    if MainThreadMarker::new().is_some() {
        return with_status_menu(|tray| update_status_node_rows(tray, title, rows, testing))
            .unwrap_or(false);
    }
    let title = title.to_owned();
    let rows = rows.to_vec();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    if handle
        .run_on_main_thread(move || {
            let updated =
                with_status_menu(|tray| update_status_node_rows(tray, &title, &rows, testing))
                    .unwrap_or(false);
            let _ = tx.send(updated);
        })
        .is_err()
    {
        return false;
    }
    rx.recv().unwrap_or(false)
}

fn update_status_node_rows(
    tray: &StatusMenu,
    title: &str,
    rows: &[TrayNodeRow],
    testing: bool,
) -> bool {
    tray.ivars()
        .item
        .menu(MainThreadMarker::from(tray))
        .is_some_and(|root| update_node_rows(&root, title, rows, testing))
}

fn update_node_rows(root: &NSMenu, title: &str, rows: &[TrayNodeRow], testing: bool) -> bool {
    let Some((parent, submenu)) = root.itemArray().iter().find_map(|parent| {
        let submenu = parent.submenu()?;
        is_node_menu(&submenu).then_some((parent, submenu))
    }) else {
        return rows.is_empty();
    };
    let items = submenu.itemArray();
    if items.len() != rows.len() + 2
        || !items.iter().skip(2).zip(rows).all(|(item, row)| {
            item_id(&item)
                .as_deref()
                .and_then(|id| id.strip_prefix("node-"))
                == Some(row.name.as_str())
        })
    {
        return false;
    }

    parent.setTitle(&NSString::from_str(&format_flag_spacing(title)));
    submenu.setMinimumWidth(NODE_MENU_WIDTH);
    let mtm = MainThreadMarker::from(root);
    let action = items.objectAtIndex(0);
    let chinese = action.title().to_string().starts_with("延迟");
    let text = match (testing, chinese) {
        (true, true) => "延迟测速中…",
        (false, true) => "延迟测速",
        (true, false) => "Testing latency…",
        (false, false) => "Test latency",
    };
    action.setTitle(&NSString::from_str(text));
    action.setEnabled(!testing);
    let view = action
        .view()
        .and_then(|view| view.downcast::<TrayActionRowView>().ok())
        .unwrap_or_else(|| make_action_row_view(mtm));
    update_action_row_view(&view, text, !testing, NODE_MENU_WIDTH);
    action.setView(Some(&view));

    for (item, row) in items.iter().skip(2).zip(rows) {
        let view = item
            .view()
            .and_then(|view| view.downcast::<TrayNodeRowView>().ok())
            .unwrap_or_else(|| make_node_row_view(mtm));
        update_node_row_view(&view, row, row.ms, NODE_MENU_WIDTH, mtm);
        item.setView(Some(&view));
        item.setState(isize::from(row.selected));
    }
    true
}
