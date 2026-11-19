//! 当前 Wi-Fi SSID 读取（#566 网络场景）。
//!
//! macOS 用 networksetup 家族命令（与系统代理 backend 同源，零新增依赖）：
//! macOS 14+ 未授予「定位服务」时拿不到真实 SSID（返回 `<wifi-ssid>` 占位
//! 或失败）——一律按未授权处理返回 None，场景功能保持不干预，设置页给出
//! 授权提示。Windows/Linux 的 SSID 读取随各平台验收单独补。

/// 当前关联的 Wi-Fi SSID；无 Wi-Fi、接口探测失败或定位权限缺失时为 None。
pub(crate) fn current_ssid() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let device = wifi_device()?;
        let output = std::process::Command::new("/usr/sbin/networksetup")
            .arg("-getairportnetwork")
            .arg(&device)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        parse_current_ssid(&String::from_utf8_lossy(&output.stdout))
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// macOS：Wi-Fi 硬件接口设备名（enX），来自 `-listallhardwareports` 的
/// 「Wi-Fi」块。输出标签未被系统本地化。
#[cfg(target_os = "macos")]
fn wifi_device() -> Option<String> {
    let output = std::process::Command::new("/usr/sbin/networksetup")
        .arg("-listallhardwareports")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_wifi_device(&String::from_utf8_lossy(&output.stdout))
}

/// 从 `-getairportnetwork` 输出解析 SSID：
/// `Current Wi-Fi Network: <name>`；未关联或无定位权限（`<wifi-ssid>`
/// 占位符）时返回 None。
#[cfg(target_os = "macos")]
fn parse_current_ssid(text: &str) -> Option<String> {
    let line = text.lines().next()?.trim();
    let ssid = line.strip_prefix("Current Wi-Fi Network: ")?.trim();
    if ssid.is_empty() || ssid.starts_with('<') {
        return None;
    }
    Some(ssid.to_string())
}

/// 从 `-listallhardwareports` 输出解析 Wi-Fi 设备名：找
/// `Hardware Port: Wi-Fi` 块内相邻的 `Device: enX` 行。
#[cfg(target_os = "macos")]
fn parse_wifi_device(text: &str) -> Option<String> {
    let mut port: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Hardware Port: ") {
            port = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("Device: ") {
            if port.as_deref() == Some("Wi-Fi") {
                return Some(rest.trim().to_string());
            }
            port = None;
        }
    }
    None
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn parse_wifi_device_picks_wifi_block_only() {
        let text = "Hardware Port: Thunderbolt Ethernet\nDevice: en5\n\n\
                    Hardware Port: Wi-Fi\nDevice: en0\n\n\
                    Hardware Port: USB 10/100/1000 LAN\nDevice: en7";
        assert_eq!(parse_wifi_device(text), Some("en0".to_string()));
        assert_eq!(parse_wifi_device("Hardware Port: Wi-Fi\n"), None);
    }

    #[test]
    fn parse_current_ssid_rejects_placeholder_and_offline() {
        assert_eq!(
            parse_current_ssid("Current Wi-Fi Network: Home-5G\n"),
            Some("Home-5G".to_string())
        );
        // 无定位权限时的占位符
        assert_eq!(
            parse_current_ssid("Current Wi-Fi Network: <wifi-ssid>\n"),
            None
        );
        assert_eq!(
            parse_current_ssid("You are not associated with an AirPort network.\n"),
            None
        );
        assert_eq!(parse_current_ssid(""), None);
    }
}
