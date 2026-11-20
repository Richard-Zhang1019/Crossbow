//! 当前 Wi-Fi SSID 读取（#566 网络场景）。
//!
//! macOS 用 networksetup 家族命令（与系统代理 backend 同源，零新增依赖）：
//! macOS 14+ 未授予「定位服务」时拿不到真实 SSID（返回 `<wifi-ssid>` 占位
//! 或失败）——一律按未授权处理返回 None，场景功能保持不干预，设置页给出
//! 授权提示。Windows/Linux 的 SSID 读取随各平台验收单独补。

#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) enum Observation {
    Known(String),
    Disconnected,
    Unavailable,
}

pub(crate) fn observe() -> Observation {
    #[cfg(target_os = "macos")]
    {
        let Some(device) = wifi_device() else {
            return Observation::Unavailable;
        };
        let output = std::process::Command::new("/usr/sbin/networksetup")
            .args(["-getairportnetwork", &device])
            .output();
        if let Ok(output) = output {
            if output.status.success() {
                if let Some(name) = parse_current_ssid(&String::from_utf8_lossy(&output.stdout)) {
                    return Observation::Known(name);
                }
            }
        }
        // Privacy-denied SSID reads can resemble an unassociated Wi-Fi. Only
        // restore on physical-link inactivity; otherwise keep current takeover.
        let inactive = std::process::Command::new("/sbin/ifconfig")
            .arg(device)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .is_some_and(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .any(|l| l.trim() == "status: inactive")
            });
        if inactive {
            Observation::Disconnected
        } else {
            Observation::Unavailable
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Observation::Unavailable
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
    let line = text.lines().next()?.trim_end_matches('\r');
    let ssid = line.strip_prefix("Current Wi-Fi Network: ")?;
    if ssid.trim().is_empty() || ssid == "<wifi-ssid>" {
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
        assert_eq!(
            parse_current_ssid("Current Wi-Fi Network:  Home \n"),
            Some(" Home ".to_string())
        );
        assert_eq!(
            parse_current_ssid("Current Wi-Fi Network: <Home>\n"),
            Some("<Home>".to_string())
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
