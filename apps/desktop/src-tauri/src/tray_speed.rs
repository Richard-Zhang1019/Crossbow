//! macOS 菜单栏实时速率：NSStatusItem 富文本双行标题（借鉴 Clash Verge Rev 的
//! 公开技术方案，实现独立编写）。
//!
//! 系统菜单栏高 24pt，默认字号塞不下两行；方案：9pt 等宽字体 + 固定 10pt 行高 +
//! 精确基线偏移把两行垂直居中在按钮内，并按内容宽度设定 status item 长度以避免
//! 每秒抖动。仅 macOS，其余平台为空操作。

use crate::tray_state::TrayNodeRow;
use tauri::{AppHandle, Manager};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, DeclaredClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::NSAttributedStringNSStringDrawing as _;
use objc2_app_kit::{
    NSApplication, NSBaselineOffsetAttributeName, NSColor, NSEvent, NSFont, NSFontAttributeName,
    NSFontWeightMedium, NSFontWeightRegular, NSForegroundColorAttributeName, NSImageView,
    NSLineBreakMode, NSMenu, NSMenuDelegate, NSMenuDidBeginTrackingNotification, NSMenuItem,
    NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSStatusBarButton, NSStatusItem,
    NSTextAlignment, NSTextField, NSView,
};
use objc2_foundation::{
    NSAttributedString, NSAttributedStringKey, NSDictionary, NSNotification, NSNotificationCenter,
    NSNumber, NSObject, NSObjectProtocol, NSString,
};
use std::sync::{Mutex, OnceLock};

const FONT_SIZE: f64 = 9.5;
const LINE_HEIGHT: f64 = 10.0;
// 图标占位 + 系统内边距（Tauri 图标 18pt）。
const EXTRA_WIDTH: f64 = 30.0;
// 短值（如 ↓0B↑0B）防抖的最小宽度。
const MIN_LENGTH: f64 = 56.0;
const NS_VARIABLE_LENGTH: f64 = -1.0;

thread_local! {
    static MENU_OBSERVER: std::cell::RefCell<Option<Retained<TrayMenuObserver>>> = const { std::cell::RefCell::new(None) };
}

/// Tray ticks arrive on a websocket reader thread while clear() can run on a
/// core lifecycle thread. Keep this cache process-wide so those paths cannot
/// disagree about the last title and leave a stale rate rendered in the menu bar.
static LAST_SPEED: OnceLock<Mutex<String>> = OnceLock::new();

fn last_speed() -> &'static Mutex<String> {
    LAST_SPEED.get_or_init(|| Mutex::new(String::new()))
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

    // tray-icon routes status-item clicks through a transparent TrayTarget
    // subview created at the button's original icon-only size. Changing the
    // NSStatusItem length does not resize that subview, so the added title area
    // looks clickable but misses menu events. Keep the hit target covering the
    // full button whenever its title/length changes.
    if let Some(target) = button.subviews().firstObject() {
        target.setFrame(button.bounds());
    }
}

// 更新托盘速率双行显示。
pub fn set_speed(handle: &AppHandle, up: u64, down: u64) {
    let text = speed_text(up, down);
    {
        let last = last_speed().lock().unwrap_or_else(|e| e.into_inner());
        if last.as_str() == text {
            return;
        }
    }
    let text_for_apply = text.clone();
    if handle
        .tray_by_id_or_first(move |item| apply(item, &text_for_apply, true))
        .is_ok()
    {
        *last_speed().lock().unwrap_or_else(|e| e.into_inner()) = text;
    }
}

// 清空速率显示（内核停止等场景），status item 宽度恢复自适应。
pub fn clear(handle: &AppHandle) {
    *last_speed().lock().unwrap_or_else(|e| e.into_inner()) = String::new();
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
        let tray = state.tray.lock().unwrap_or_else(|e| e.into_inner()).clone();
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

const ROW_VIEW_NAME_TAG: isize = 101;
const ROW_VIEW_CHECK_TAG: isize = 102;
const ROW_VIEW_BADGE_TAG: isize = 103;
const ROW_VIEW_CURRENT_TAG: isize = 107;
const ACTION_ROW_VIEW_LABEL_TAG: isize = 104;
const NODE_MENU_WIDTH: f64 = 270.0;
const NODE_ROW_HEIGHT: f64 = 23.0;
const ACTION_ROW_HEIGHT: f64 = NODE_ROW_HEIGHT;
const HOVER_HORIZONTAL_INSET: f64 = 6.0;
const HOVER_VERTICAL_INSET: f64 = 1.0;
const ROW_CONTENT_INSET: f64 = 6.0;
const HOVER_CORNER_RADIUS: f64 = 7.0;

// NSMenuItem custom views do not automatically dispatch the menu item's action.
// Forward the click to the original target/action so Tauri still receives the
// same menu event as it does for a standard check-menu item.
define_class!(
    #[unsafe(super(NSView))]
    #[name = "CrossbowTrayNodeRowView"]
    #[thread_kind = MainThreadOnly]
    struct TrayNodeRowView;

    unsafe impl NSObjectProtocol for TrayNodeRowView {}

    impl TrayNodeRowView {
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

    view.addSubview(&check);
    view.addSubview(&name);
    view.addSubview(&current);
    view.addSubview(&badge);
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

    let badge_width = (display_width(&row.badge) as f64 * 6.0 + 10.0).max(28.0);
    let badge_x = menu_width - badge_width - HOVER_HORIZONTAL_INSET - ROW_CONTENT_INSET;
    let current_text = row
        .current_node
        .as_ref()
        .map(|node| format_flag_spacing(node));
    let name_width = if current_text.is_some() {
        (display_width(&row.label) as f64 * 5.8 + 4.0).clamp(66.0, 88.0)
    } else {
        (badge_x - 38.0).max(50.0)
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
        let current_width = (badge_x - current_x - 5.0).max(0.0);
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
    badge.setAlignment(NSTextAlignment::Center);
    set_menu_label_text(&badge, &row.badge);
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
    }
}

fn install_menu_delegate(menu: &NSMenu, delegate: &TrayMenuObserver) {
    menu.setDelegate(Some(ProtocolObject::from_ref(delegate)));
    for index in 0..menu.numberOfItems() {
        if let Some(submenu) = menu.itemAtIndex(index).and_then(|item| item.submenu()) {
            install_menu_delegate(&submenu, delegate);
        }
    }
}

fn layer_for(view: &NSView) -> Option<Retained<AnyObject>> {
    unsafe { msg_send![view, layer] }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "CrossbowTrayMenuObserver"]
    #[thread_kind = MainThreadOnly]
    #[ivars = AppHandle]
    struct TrayMenuObserver;

    unsafe impl NSObjectProtocol for TrayMenuObserver {}

    unsafe impl NSMenuDelegate for TrayMenuObserver {
        #[unsafe(method(menu:willHighlightItem:))]
        #[allow(non_snake_case)]
        fn menu_willHighlightItem(&self, menu: &NSMenu, highlighted: Option<&NSMenuItem>) {
            for index in 0..menu.numberOfItems() {
                let Some(item) = menu.itemAtIndex(index) else { continue };
                let is_highlighted = highlighted.is_some_and(|candidate| {
                    std::ptr::eq(&*item, candidate)
                });
                set_menu_item_hovered(&item, is_highlighted);
            }
        }
    }

    impl TrayMenuObserver {
        #[unsafe(method(menuDidBeginTracking:))]
        fn menu_did_begin_tracking(&self, notification: &NSNotification) {
            let Some(menu) = notification
                .object()
                .and_then(|object| object.downcast::<NSMenu>().ok())
            else {
                return;
            };
            let handle = self.ivars();
            // 非节点菜单（如模式子菜单）：勾选态随当前模式刷新
            if !contains_delay_test(&menu) {
                update_mode_checkmarks(&menu, handle);
                return;
            }
            install_menu_delegate(&menu, self);
            let Some(state) = handle.try_state::<crate::AppState>() else {
                return;
            };
            let lang = state.store.lock().unwrap().data().ui.lang.clone();
            let data = state.node_menu.lock().unwrap_or_else(|e| e.into_inner()).snapshot(
                &crate::tr(&lang, "节点选择", "Nodes"),
                &crate::tr(&lang, "当前", "Current"),
            );
            let (menu_width, badge_tab) = node_menu_layout(&data.rows);
            update_menu_items(&menu, &data.rows, data.testing, menu_width, badge_tab);
            // 状态项文字（系统代理开/关、当前模式）：菜单关闭期间的
            // refresh_tray 触达不到 NSStatusItem 的菜单，只能在展开瞬间刷新
            update_state_labels(&menu, handle);
            let is_node_submenu = (0..menu.numberOfItems()).any(|index| {
                menu.itemAtIndex(index)
                    .is_some_and(|item| is_delay_test_item(&item))
            });
            if is_node_submenu {
                crate::refresh_tray_nodes(handle, true);
            }
        }
    }
);

/// Install rows at the beginning of AppKit menu tracking. Tauri temporarily
/// assigns the menu to NSStatusItem only while tracking, so refreshing it while
/// closed cannot reach the menu; this notification runs before the first rows
/// are displayed and keeps custom badges/actions ready on every opening.
pub fn install_menu_observer(handle: &AppHandle) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let observer = TrayMenuObserver::alloc(mtm).set_ivars(handle.clone());
    let observer: Retained<TrayMenuObserver> = unsafe { msg_send![super(observer), init] };
    let center = NSNotificationCenter::defaultCenter();
    unsafe {
        center.addObserver_selector_name_object(
            &*observer,
            objc2::sel!(menuDidBeginTracking:),
            Some(NSMenuDidBeginTrackingNotification),
            None,
        );
    }
    MENU_OBSERVER.with(|current| *current.borrow_mut() = Some(observer));
}

pub fn display_name(name: &str) -> String {
    truncate_display_name(&format_flag_spacing(name), 202.0)
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

// 原地更新打开中的托盘节点菜单（不重建菜单，所以选择后可继续操作）。
pub fn update_node_menu(
    handle: &AppHandle,
    title: &str,
    rows: &[TrayNodeRow],
    testing: bool,
) -> bool {
    let state = handle.state::<crate::AppState>();
    let tray = state.tray.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Some(tray) = tray else { return false };
    let rows_owned: Vec<TrayNodeRow> = rows.to_vec();
    let title_owned = title.to_string();
    let testing_owned = testing;
    let updated = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let updated_in_callback = updated.clone();
    let _ = tray.with_inner_tray_icon(move |inner| {
        let Some(status_item) = inner.ns_status_item() else {
            return;
        };
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(menu) = status_item.menu(mtm) else {
            return;
        };
        let has_node_menu = update_node_parent_title(&menu, &title_owned);
        let (menu_width, badge_tab) = node_menu_layout(&rows_owned);
        update_menu_items(&menu, &rows_owned, testing_owned, menu_width, badge_tab);
        updated_in_callback.store(has_node_menu, std::sync::atomic::Ordering::SeqCst);
    });
    updated.load(std::sync::atomic::Ordering::SeqCst)
}

fn update_node_parent_title(menu: &NSMenu, title: &str) -> bool {
    for index in 0..menu.numberOfItems() {
        let Some(item) = menu.itemAtIndex(index) else {
            continue;
        };
        if let Some(submenu) = item.submenu() {
            if contains_delay_test(&submenu) {
                item.setTitle(&NSString::from_str(&format_flag_spacing(title)));
                return true;
            }
            if update_node_parent_title(&submenu, title) {
                return true;
            }
        }
    }
    false
}

fn is_delay_test_item(item: &NSMenuItem) -> bool {
    matches!(
        item.title().to_string().as_str(),
        "延迟测速" | "延迟测速中…" | "Test latency" | "Testing latency…"
    )
}

fn contains_delay_test(menu: &NSMenu) -> bool {
    (0..menu.numberOfItems()).any(|index| {
        menu.itemAtIndex(index).is_some_and(|item| {
            is_delay_test_item(&item)
                || item
                    .submenu()
                    .is_some_and(|submenu| contains_delay_test(&submenu))
        })
    })
}

/// 菜单展开瞬间同步状态项文字（系统代理开/关、当前模式标题）。
/// 标题按 build_tray_menu 的格式重建：切换可来自 UI 或托盘任一侧，
/// 以当前 AppState 为准原位刷新。
fn update_state_labels(menu: &NSMenu, handle: &AppHandle) {
    let Some(state) = handle.try_state::<crate::AppState>() else {
        return;
    };
    let (lang, tun_enabled) = {
        let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        (store.data().ui.lang.clone(), store.data().engine.tun_enable)
    };
    let sp = state.sysproxy.status();
    let mode = state.mode.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let proxy_title = if tun_enabled {
        if state.core().status() == crate::CoreStatus::Running {
            crate::tr(&lang, "TUN 模式：运行中", "TUN mode: Running")
        } else {
            crate::tr(&lang, "TUN 已开启 · 内核未运行", "TUN enabled · core stopped")
        }
    } else if sp.enabled {
        format!(
            "{} :{}",
            crate::tr(&lang, "系统代理：已开启", "System Proxy: On"),
            sp.port
        )
    } else {
        crate::tr(&lang, "系统代理：已关闭", "System Proxy: Off")
    };
    let mode_title = format!(
        "{}：{}",
        crate::tr(&lang, "模式", "Mode"),
        crate::tr(
            &lang,
            match mode.as_str() {
                "direct" => "直连",
                "global" => "全局",
                _ => "规则",
            },
            match mode.as_str() {
                "direct" => "Direct",
                "global" => "Global",
                _ => "Rule",
            }
        )
    );
    for index in 0..menu.numberOfItems() {
        let Some(item) = menu.itemAtIndex(index) else {
            continue;
        };
        let title = item.title().to_string();
        let proxy_status_item = title.starts_with("系统代理")
            || title.starts_with("System Proxy")
            || title.starts_with("TUN ");
        let new_title = if proxy_status_item {
            Some(proxy_title.clone())
        } else if title.starts_with("模式") || title.starts_with("Mode") {
            Some(mode_title.clone())
        } else {
            None
        };
        if let Some(new_title) = new_title {
            item.setTitle(&NSString::from_str(&new_title));
            item.setEnabled(!tun_enabled);
        }
    }
}

/// 模式子菜单的勾选态随当前模式刷新（直连/规则/全局）。
fn update_mode_checkmarks(menu: &NSMenu, handle: &AppHandle) {
    let Some(state) = handle.try_state::<crate::AppState>() else {
        return;
    };
    let lang = state.store.lock().unwrap_or_else(|e| e.into_inner()).data().ui.lang.clone();
    let mode = state.mode.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let marks: [(&str, &str, bool); 3] = [
        (
            &crate::tr(&lang, "直连", "Direct"),
            "direct",
            mode == "direct",
        ),
        (&crate::tr(&lang, "规则", "Rule"), "rule", mode == "rule"),
        (
            &crate::tr(&lang, "全局", "Global"),
            "global",
            mode == "global",
        ),
    ];
    for index in 0..menu.numberOfItems() {
        let Some(item) = menu.itemAtIndex(index) else {
            continue;
        };
        let title = item.title().to_string();
        if let Some((_, _, on)) = marks.iter().find(|(label, _, _)| *label == title) {
            // NSControlStateValueOn=1 / Off=0，避开 NSControl feature 依赖
            let _: () = unsafe { msg_send![&*item, setState: (*on) as isize] };
        }
    }
}

fn node_menu_layout(_rows: &[TrayNodeRow]) -> (f64, f64) {
    // Fixed menu width and right edge for the badge column. Long node names are
    // truncated to keep this width stable after every benchmark result.
    (NODE_MENU_WIDTH, NODE_MENU_WIDTH - 30.0)
}

fn update_menu_items(
    menu: &NSMenu,
    rows: &[TrayNodeRow],
    testing: bool,
    menu_width: f64,
    badge_tab: f64,
) {
    let is_node_menu = (0..menu.numberOfItems()).any(|index| {
        menu.itemAtIndex(index)
            .is_some_and(|item| is_delay_test_item(&item))
    });
    if is_node_menu {
        menu.setMinimumWidth(menu_width);
    }

    let mut row_index = 0;
    for index in 0..menu.numberOfItems() {
        let Some(item) = menu.itemAtIndex(index) else {
            continue;
        };
        let title = item.title().to_string();
        if is_delay_test_item(&item) {
            let chinese = title.starts_with("延迟");
            let text = match (testing, chinese) {
                (true, true) => "延迟测速中…",
                (false, true) => "延迟测速",
                (true, false) => "Testing latency…",
                (false, false) => "Test latency",
            };
            item.setTitle(&NSString::from_str(text));
            item.setEnabled(!testing);
            let mtm = MainThreadMarker::new().expect("tray menu updates run on the main thread");
            let row_view = item
                .view()
                .and_then(|view| view.downcast::<TrayActionRowView>().ok())
                .unwrap_or_else(|| make_action_row_view(mtm));
            update_action_row_view(&row_view, text, !testing, menu_width);
            item.setView(Some(&row_view));
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
                let mtm =
                    MainThreadMarker::new().expect("tray menu updates run on the main thread");
                let row_view = item
                    .view()
                    .and_then(|view| view.downcast::<TrayNodeRowView>().ok())
                    .unwrap_or_else(|| make_node_row_view(mtm));
                update_node_row_view(&row_view, row, row.ms, menu_width, mtm);
                item.setView(Some(&row_view));
                item.setState(if row.selected { 1 } else { 0 });
            }
        }

        if let Some(submenu) = item.submenu() {
            update_menu_items(&submenu, rows, testing, menu_width, badge_tab);
        }
    }
}
