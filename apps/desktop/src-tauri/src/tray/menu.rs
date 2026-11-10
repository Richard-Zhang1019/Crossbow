//! Shared tray menu entries; AppKit owns the status item on macOS.

#[cfg_attr(target_os = "macos", derive(serde::Deserialize, serde::Serialize))]
pub(crate) enum Entry {
    Action {
        id: String,
        title: String,
        enabled: bool,
        checked: Option<bool>,
    },
    Submenu(String, Vec<Entry>),
    Separator,
}

impl Entry {
    pub(crate) fn action(id: impl Into<String>, title: String, enabled: bool) -> Self {
        Self::Action {
            id: id.into(),
            title,
            enabled,
            checked: None,
        }
    }

    pub(crate) fn check(id: impl Into<String>, title: String, checked: bool) -> Self {
        Self::Action {
            id: id.into(),
            title,
            enabled: true,
            checked: Some(checked),
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn build(
    app: &tauri::AppHandle,
    entries: &[Entry],
) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{
        CheckMenuItem, MenuBuilder, MenuItem, MenuItemKind, PredefinedMenuItem, SubmenuBuilder,
    };

    fn item(app: &tauri::AppHandle, entry: &Entry) -> tauri::Result<MenuItemKind<tauri::Wry>> {
        Ok(match entry {
            Entry::Action {
                id,
                title,
                enabled,
                checked: Some(checked),
            } => MenuItemKind::Check(CheckMenuItem::with_id(
                app,
                id.as_str(),
                title,
                *enabled,
                *checked,
                None::<&str>,
            )?),
            Entry::Action {
                id,
                title,
                enabled,
                checked: None,
            } => MenuItemKind::MenuItem(MenuItem::with_id(
                app,
                id.as_str(),
                title,
                *enabled,
                None::<&str>,
            )?),
            Entry::Submenu(title, entries) => {
                let mut builder = SubmenuBuilder::new(app, title);
                for entry in entries {
                    builder = builder.item(&item(app, entry)?);
                }
                MenuItemKind::Submenu(builder.build()?)
            }
            Entry::Separator => MenuItemKind::Predefined(PredefinedMenuItem::separator(app)?),
        })
    }

    let mut builder = MenuBuilder::new(app);
    for entry in entries {
        builder = builder.item(&item(app, entry)?);
    }
    builder.build()
}
