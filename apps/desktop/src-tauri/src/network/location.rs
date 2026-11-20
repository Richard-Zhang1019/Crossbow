//! 定位服务授权状态与请求（#566 网络场景）。
//!
//! macOS 14+ 读取 SSID 需要应用持有定位授权，且系统**不会**把应用自动
//! 登记进「隐私与安全性 → 定位服务」列表——必须由应用显式发起
//! `requestWhenInUseAuthorization`，首次会弹系统授权框，用户允许后
//! Crossbow 才出现在列表里（否则 networksetup 只返回 `<wifi-ssid>`
//! 占位符，场景功能永远读不到真实 SSID）。非 macOS 平台一律 unsupported。

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LocationPermission {
    /// 已授权（always 或 when-in-use）。
    Authorized,
    /// 被用户或系统策略拒绝：应用已出现在定位服务列表，引导去设置开启。
    Denied,
    /// 尚未发起过请求：显示「请求定位权限」入口，挂载时自动触发一次。
    NotDetermined,
    /// 非 macOS 平台：场景功能不依赖定位（SSID 探测恒 None）。
    #[allow(dead_code)] // macOS 编译时该变体无构造点
    Unsupported,
}

#[cfg(target_os = "macos")]
#[cfg(target_os = "macos")]
fn map_status(raw: i32) -> LocationPermission {
    // CLAuthorizationStatus：0 notDetermined / 1 restricted / 2 denied /
    // 3 authorizedAlways / 4 authorizedWhenInUse
    match raw {
        0 => LocationPermission::NotDetermined,
        3 | 4 => LocationPermission::Authorized,
        // restricted（MDM/家长控制）与 denied 同属「去设置也没有」面
        _ => LocationPermission::Denied,
    }
}

/// 当前定位授权状态（不触发弹窗）。CLLocationManager 可在任意线程创建。
#[cfg(target_os = "macos")]
pub(crate) fn status() -> LocationPermission {
    let manager = unsafe { objc2_core_location::CLLocationManager::new() };
    map_status(unsafe { manager.authorizationStatus() }.0)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn status() -> LocationPermission {
    LocationPermission::Unsupported
}

/// 发起 when-in-use 授权请求（首次会弹系统授权框），返回请求后的当前
/// 状态（弹窗未决时仍为 not_determined）。CLLocationManager 允许任意
/// 线程创建，无需主线程标记。
#[cfg(target_os = "macos")]
pub(crate) fn request_when_in_use() -> LocationPermission {
    let manager = unsafe { objc2_core_location::CLLocationManager::new() };
    unsafe { manager.requestWhenInUseAuthorization() };
    status()
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn request_when_in_use(_app: &tauri::AppHandle) -> LocationPermission {
    LocationPermission::Unsupported
}
