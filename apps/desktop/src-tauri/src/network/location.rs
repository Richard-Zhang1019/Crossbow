//! macOS Wi-Fi 场景所需的定位授权。请求对象由主线程持有。

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) enum LocationPermission {
    Authorized,
    Denied,
    NotDetermined,
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Unsupported,
}

#[cfg(target_os = "macos")]
fn map_status(raw: i32) -> LocationPermission {
    match raw {
        0 => LocationPermission::NotDetermined,
        3 | 4 => LocationPermission::Authorized,
        _ => LocationPermission::Denied,
    }
}

#[cfg(target_os = "macos")]
thread_local! {
    static MANAGER: objc2::rc::Retained<objc2_core_location::CLLocationManager> =
        unsafe { objc2_core_location::CLLocationManager::new() };
}

/// 查询全局授权状态，不在工作线程创建要求 RunLoop 的 manager。
#[cfg(target_os = "macos")]
#[allow(deprecated)]
pub(crate) fn status() -> LocationPermission {
    map_status(unsafe { objc2_core_location::CLLocationManager::authorizationStatus_class() }.0)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn status() -> LocationPermission {
    LocationPermission::Unsupported
}

/// 保持 manager 存活至异步授权结束。此函数仅在主线程调用。
#[cfg(target_os = "macos")]
pub(crate) fn request_when_in_use() -> LocationPermission {
    MANAGER.with(|manager| {
        unsafe { manager.requestWhenInUseAuthorization() };
        map_status(unsafe { manager.authorizationStatus() }.0)
    })
}
