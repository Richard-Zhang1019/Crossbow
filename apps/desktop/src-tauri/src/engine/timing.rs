//! 行为节奏常量集中治理（#426）。
//!
//! #350/#351 之后 TUN 健康看门狗与系统代理死亡看门狗共用同一轮询循环，
//! 两套计数必须引用同一处节奏常量，避免常量分离后调参不同步。跨模块
//! 共享的看门狗节奏集中于此；单一域内部的超时留在各域模块的 const 区具名。

use std::time::Duration;

/// 看门狗轮询周期：TUN 健康/系统代理死亡、托盘页面通道、托盘测速通道
/// 三个看门狗共用（#331/#181/#291）。
pub const WATCHDOG_TICK: Duration = Duration::from_secs(5);

/// 连续失败/死亡周期预算：TUN 健康检查连续 3 次失败关闭 TUN（#350）；
/// 系统代理模式下按同一预算累计（3 × WATCHDOG_TICK = 15s）后关闭系统
/// 代理还原直连（#356）。
pub const FAILURE_BUDGET: u8 = 3;

/// TUN 栈楔死自动重启预算（#350）。
pub const WEDGE_RESTART_BUDGET: u8 = 2;

/// TUN 栈楔死重启冷却（#350/#351）。
pub const WEDGE_COOLDOWN: Duration = Duration::from_secs(90);

/// 接管漂移关开重建立冷却（#154）。
pub const REESTABLISH_COOLDOWN: Duration = Duration::from_secs(120);

/// 托盘页面通道（traffic/connections）静默强制重订阈值（#181）。
pub const PAGE_CHANNEL_STALE: Duration = Duration::from_secs(15);

/// 托盘测速通道（traffic-tray）静默强制重订阈值（#291 系列）。
pub const TRAY_SPEED_CHANNEL_STALE: Duration = Duration::from_secs(12);

/// TUN 监听器就绪等待上限：控制器就绪不代表路由注入完成（macOS utun，#331）。
pub const TUN_LISTENER_DEADLINE: Duration = Duration::from_secs(10);

/// TUN 监听器就绪轮询间隔。
pub const TUN_LISTENER_POLL: Duration = Duration::from_millis(100);

/// Windows TUN 适配器就绪等待上限（wintun 建立通常 <1s，服务拉起内核
/// 与探测窗口兜底 15s）。
pub const WINDOWS_TUN_READY_DEADLINE: Duration = Duration::from_secs(15);
