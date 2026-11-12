//! 应用退出与会话生命周期：退出状态机、授权、平台会话结束回调。
pub mod authorization;
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(crate) mod exit_flow;
#[cfg(target_os = "macos")]
pub mod macos_shutdown;
pub mod shutdown;
#[cfg(windows)]
pub mod windows_session_end;
