//! 订阅获取与基础解析（MVP：URL 拉取 + 内容落库 + 流量头解析）。

use std::time::{SystemTime, UNIX_EPOCH};

use crate::model::TrafficInfo;
use crate::{Error, Result};

/// 一次订阅拉取的结果。
pub struct SubscriptionFetch {
    /// 订阅原文（Clash/mihomo YAML，或 base64 分享文本——后者由上游转换，S2 只支持 YAML）。
    pub content: String,
    /// `subscription-userinfo` 响应头解析出的流量信息；无则 None。
    pub traffic: Option<TrafficInfo>,
}

/// 拉取订阅原文与流量头。带超时与 UA（部分机场对非浏览器 UA 返回压缩/报错内容）。
pub fn fetch_subscription(url: &str, timeout_secs: u64) -> Result<SubscriptionFetch> {
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .user_agent("Crossbow/0.1 (clash-verge-compatible)")
        .build();
    let resp = agent
        .get(url)
        .call()
        .map_err(|e| Error::Network(format!("GET {url}: {e}")))?;
    if resp.status() >= 400 {
        return Err(Error::Network(format!("GET {url}: HTTP {}", resp.status())));
    }
    let traffic = resp
        .header("subscription-userinfo")
        .and_then(parse_userinfo);
    let content = resp
        .into_string()
        .map_err(|e| Error::Network(format!("read body: {e}")))?;
    Ok(SubscriptionFetch { content, traffic })
}

/// 解析 `subscription-userinfo` 头，如
/// `upload=123; download=4567; total=10737418240; expire=1758988800`。
pub fn parse_userinfo(header: &str) -> Option<TrafficInfo> {
    let mut info = TrafficInfo::default();
    let mut seen = false;
    for part in header.split(';') {
        let Some((k, v)) = part.trim().split_once('=') else {
            continue;
        };
        let Ok(v) = v.trim().parse::<u64>() else {
            continue;
        };
        match k.trim().to_ascii_lowercase().as_str() {
            "upload" => {
                info.upload = v;
                seen = true;
            }
            "download" => {
                info.download = v;
                seen = true;
            }
            "total" => {
                info.total = v;
                seen = true;
            }
            "expire" => {
                info.expire = v;
                seen = true;
            }
            _ => {}
        }
    }
    seen.then_some(info)
}

/// 当前 Unix 秒。
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 导入/更新前的基础校验：必须是能解析的 YAML 且包含 `proxies` 段。
pub fn validate_profile_content(content: &str) -> Result<()> {
    parse_proxy_names(content)?;
    Ok(())
}

/// 从 Clash/mihomo YAML 中提取节点名列表（仅用于 UI 展示与校验，
/// 内核启动仍直接消费原文，避免重复实现内核语义）。
pub fn parse_proxy_names(clash_yaml: &str) -> Result<Vec<String>> {
    let doc: serde_yaml::Value = serde_yaml::from_str(clash_yaml)?;
    let proxies = doc
        .get("proxies")
        .and_then(|v| v.as_sequence())
        .ok_or_else(|| Error::Network("config has no `proxies` section".into()))?;
    Ok(proxies
        .iter()
        .filter_map(|p| p.get("name").and_then(|n| n.as_str()).map(String::from))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
port: 7890
proxies:
  - name: "HK-01"
    type: ss
    server: a.example.com
    port: 8388
    cipher: aes-128-gcm
    password: x
  - name: "US-01"
    type: trojan
    server: b.example.com
    port: 443
    password: y
"#;

    #[test]
    fn parses_proxy_names() {
        let names = parse_proxy_names(SAMPLE).unwrap();
        assert_eq!(names, vec!["HK-01".to_string(), "US-01".to_string()]);
    }

    #[test]
    fn rejects_config_without_proxies() {
        assert!(parse_proxy_names("port: 7890").is_err());
    }

    #[test]
    fn parses_userinfo_header() {
        let t = parse_userinfo("upload=123; download=4567; total=10737418240; expire=1758988800")
            .unwrap();
        assert_eq!(t.upload, 123);
        assert_eq!(t.download, 4567);
        assert_eq!(t.total, 10737418240);
        assert_eq!(t.expire, 1758988800);
    }

    #[test]
    fn userinfo_without_known_keys_is_none() {
        assert!(parse_userinfo("foo=1").is_none());
        // 部分键缺失仍可解析。
        let t = parse_userinfo("upload=5; total=9").unwrap();
        assert_eq!(t.upload, 5);
        assert_eq!(t.expire, 0);
    }

    #[test]
    fn validate_accepts_sample_rejects_garbage() {
        assert!(validate_profile_content(SAMPLE).is_ok());
        assert!(validate_profile_content("not: []").is_err());
    }
}
