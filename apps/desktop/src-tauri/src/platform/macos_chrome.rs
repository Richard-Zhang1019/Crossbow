//! macOS 原生质感（外部评审建议 2026-11-27 第 4 条）：
//! - 应用菜单：默认菜单结构上补「设置… ⌘,」（跳设置页）。⌘W 收起主窗口由
//!   Window 菜单的 Close Window 提供，走既有「关窗驻留托盘/轻量退出」链路；
//!   ⌘Q 走既有退出恢复流程。
//! - 窗口 Vibrancy：应用 sidebar 材质；透出区域由 CSS 控制（标题栏 28px 条
//!   与纸面 8px 沟槽，见 layout.css 的 is-macos/is-mac 规则）。

use tauri::{
    menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu},
    window::{Effect, EffectsBuilder},
    Emitter, Manager, WebviewWindow,
};

/// 重建 macOS 应用菜单：与 Tauri 默认菜单同构，另加「设置… ⌘,」。
pub(crate) fn install_menu(app: &tauri::AppHandle) -> tauri::Result<()> {
    let lang = app
        .state::<crate::AppState>()
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .data()
        .ui
        .lang
        .clone();
    let package = app.package_info();
    let settings = MenuItem::with_id(
        app,
        "app-settings",
        crate::tr(&lang, "设置…", "Settings…").as_str(),
        true,
        Some("CmdOrControl+,"),
    )?;
    let about = PredefinedMenuItem::about(
        app,
        None,
        Some(AboutMetadata {
            name: Some(package.name.clone()),
            version: Some(package.version.to_string()),
            ..Default::default()
        }),
    )?;
    let app_menu = Submenu::with_items(
        app,
        package.name.clone(),
        true,
        &[
            &about,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;
    let edit_menu = Submenu::with_items(
        app,
        crate::tr(&lang, "编辑", "Edit"),
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let window_menu = Submenu::with_items(
        app,
        crate::tr(&lang, "窗口", "Window"),
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;
    let menu = Menu::with_items(app, &[&app_menu, &edit_menu, &window_menu])?;
    app.set_menu(menu)?;
    app.on_menu_event(|app, event| {
        if event.id().as_ref() == "app-settings" {
            // 主窗口可能已收起到托盘或处于轻量待机：先恢复窗口再跳转，
            // 否则菜单项看起来没有反应（与托盘「设置」路径一致）。
            crate::show_main_window(app);
            let _ = app.emit("ui://goto", "settings");
        }
    });
    Ok(())
}

/// 窗口材质：sidebar 材质跟随系统外观；透出区域由 CSS 透明区决定。
pub(crate) fn install_vibrancy(win: &WebviewWindow) -> tauri::Result<()> {
    win.set_effects(EffectsBuilder::new().effect(Effect::Sidebar).build())
}
