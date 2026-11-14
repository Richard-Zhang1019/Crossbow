//! 系统网络接管：系统代理、TUN、与其他内核共存检测、站点连通性测试。
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub mod coexist;
#[cfg(target_os = "linux")]
pub mod linux_service;
pub mod network_watch;
pub mod site_tests;
pub mod sysproxy;
#[cfg(target_os = "macos")]
pub mod tun;
#[cfg(target_os = "linux")]
pub mod tun_linux;
#[cfg(windows)]
pub mod tun_windows;
