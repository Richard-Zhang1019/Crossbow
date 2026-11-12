//! 内核（mihomo / sing-box）生命周期与控制面：启停、重启、下载安装、API 访问与 WS 桥。
pub mod api;
pub mod bridge;
pub mod config_transition;
pub mod download;
pub(crate) mod flow;
pub mod manager;
pub mod restart;
pub mod timing;
