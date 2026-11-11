//! 系统托盘 / 菜单栏：菜单构建、速率显示、状态同步与平台助手。
#[cfg(target_os = "macos")]
pub mod helper;
#[cfg(target_os = "macos")]
pub mod icon;
pub mod menu;
#[cfg(target_os = "macos")]
pub mod speed;
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub mod state;
pub(crate) mod wiring;
