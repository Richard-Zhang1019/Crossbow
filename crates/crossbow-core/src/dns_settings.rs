//! #598 DNS 配置：中立意图（沿用订阅 / 受控自定义）+ 引擎映射。
//!
//! 优先级：用户自定义 > 覆写 > 订阅 DNS > TUN 运行时注入——enable/ipv6
//! 等运行参数仍由 TUN 层最后深度合并注入（覆盖式键不会清掉 nameserver
//! 类字段）。「沿用订阅」不改变任何现有行为。
//!
//! 引擎能力表（H1）：
//! | 意图 | mihomo | sing-box |
//! | 代理业务上游 | dns.nameserver | proxy-dns server（detour 主出站） |
//! | 直连业务上游 | dns.direct-nameserver | local-dns server（直连拨号） |
//! | 节点引导上游 | dns.proxy-server-nameserver | 不支持（系统解析接管，禁用+说明） |
//! | 内网/hosts | 沿用订阅 hosts 与私网直连规则（H1 不开放自定义） |
//! | IPv6 | 跟随系统原生 v6 检测（runtime 注入 ipv6），H1 不开放开关 |

use serde::{Deserialize, Serialize};

/// DNS 模式：沿用订阅（默认，零行为变更）或受控自定义。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DnsMode {
    #[default]
    Inherit,
    Custom,
}

fn default_dns_mode() -> DnsMode {
    DnsMode::Inherit
}

/// DNS 设置（存 StoreSchema.dns）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsSettings {
    #[serde(default = "default_dns_mode")]
    pub mode: DnsMode,
    /// 代理业务上游（经代理出站解析，防泄漏）
    #[serde(default)]
    pub proxy_nameservers: Vec<String>,
    /// 直连业务上游（直连出站/本地域名）
    #[serde(default)]
    pub direct_nameservers: Vec<String>,
    /// 节点引导上游（节点服务器域名的解析；仅 mihomo 支持）
    #[serde(default)]
    pub bootstrap_nameservers: Vec<String>,
}

/// 解析后的上游条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Upstream {
    pub(crate) scheme: &'static str,
    pub(crate) server: String,
    pub(crate) port: u16,
    pub(crate) path: Option<String>,
}

impl Upstream {
    fn default_port(scheme: &str) -> u16 {
        match scheme {
            "https" => 443,
            "tls" => 853,
            _ => 53,
        }
    }
}

/// 校验设置：自定义模式至少一个上游，且每条上游格式可解析。
/// 格式错误在此拒绝（前端保存前 + 渲染前双保险），失败保留旧配置。
pub fn validate(settings: &DnsSettings) -> Result<(), String> {
    if settings.mode != DnsMode::Custom {
        return Ok(());
    }
    if settings.proxy_nameservers.is_empty() && settings.direct_nameservers.is_empty() {
        return Err("自定义 DNS 至少需要一个上游（代理业务或直连业务）".into());
    }
    for list in [
        &settings.proxy_nameservers,
        &settings.direct_nameservers,
        &settings.bootstrap_nameservers,
    ] {
        for upstream in list {
            parse_upstream(upstream)?;
        }
    }
    Ok(())
}

/// 解析单条上游（中立格式，mihomo 兼容写法）：
/// `https://host[:port][/path]`、`tls://host[:port]`、`udp://host[:port]`、
/// 裸 `host` 或 `ip`（等价 udp:53）。IPv6 地址需带 scheme 与方括号。
pub(crate) fn parse_upstream(raw: &str) -> Result<Upstream, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("DNS 上游不能为空".into());
    }
    let (scheme, rest) = match s.split_once("://") {
        Some((scheme, rest)) if matches!(scheme, "https" | "tls" | "udp") => {
            // 显式选择 &'static 字面量：解构绑定携带 raw 的生命周期，
            // 直接放结构体会要求 '1 outlive 'static
            let scheme: &'static str = match scheme {
                "https" => "https",
                "tls" => "tls",
                _ => "udp",
            };
            (scheme, rest)
        }
        Some((scheme, _)) => {
            return Err(format!(
                "不支持的 DNS 上游协议「{scheme}://」（支持 https/tls/udp）"
            ));
        }
        None => ("udp", s),
    };
    // 拆路径（仅 https 有意义）
    let (authority, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], Some(rest[idx..].to_string())),
        None => (rest, None),
    };
    // host[:port]，支持 [ipv6]:port
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (v6, after) = rest
            .split_once(']')
            .ok_or_else(|| format!("DNS 上游 IPv6 地址缺少右方括号：{raw}"))?;
        let port = after
            .strip_prefix(':')
            .map(|p| {
                p.parse::<u16>()
                    .map_err(|_| format!("DNS 上游端口非法：{raw}"))
            })
            .transpose()?
            .unwrap_or_else(|| Upstream::default_port(scheme));
        (v6.to_string(), port)
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => {
                let port: u16 = port
                    .parse()
                    .map_err(|_| format!("DNS 上游端口非法：{raw}"))?;
                (host.to_string(), port)
            }
            _ => (authority.to_string(), Upstream::default_port(scheme)),
        }
    };
    if host.is_empty() {
        return Err(format!("DNS 上游缺少服务器地址：{raw}"));
    }
    Ok(Upstream {
        scheme,
        server: host,
        port,
        path: path.filter(|p| p != "/"),
    })
}

/// mihomo 侧沿用用户原始写法（内核原生解析，写法面最广）。
pub(crate) fn upstream_mihomo(raw: &str) -> String {
    raw.trim().to_string()
}

/// sing-box dns.servers 条目（1.12+ 形态）。
pub(crate) fn upstream_singbox_json(
    upstream: &Upstream,
    tag: &str,
    detour: Option<&str>,
) -> serde_json::Value {
    let mut server = serde_json::json!({
        "type": upstream.scheme,
        "tag": tag,
        "server": upstream.server,
    });
    if upstream.port != Upstream::default_port(upstream.scheme) {
        server["port"] = serde_json::json!(upstream.port);
    }
    if let Some(path) = &upstream.path {
        if upstream.scheme == "https" {
            server["path"] = serde_json::json!(path);
        }
    }
    if let Some(detour) = detour {
        server["detour"] = serde_json::json!(detour);
    }
    server
}

/// #598：自定义模式向 mihomo 渲染配置注入用户上游（深度合并，订阅 DNS
/// 其余字段——fake-ip 模式、hosts 等——保留）。沿用订阅零改动。
/// 校验先行：非法上游在渲染层报错，调用方既有事务保留旧配置。
pub fn apply_mihomo_dns_patch(config: String, dns: &DnsSettings) -> Result<String, String> {
    if dns.mode != DnsMode::Custom {
        return Ok(config);
    }
    validate(dns)?;
    // nameserver（默认解析）：代理业务优先，未填则落到直连上游
    let nameserver = if dns.proxy_nameservers.is_empty() {
        &dns.direct_nameservers
    } else {
        &dns.proxy_nameservers
    };
    let mut dns_map = serde_yaml::Mapping::new();
    let to_seq = |list: &[String]| {
        serde_yaml::Value::Sequence(
            list.iter()
                .map(|raw| serde_yaml::Value::from(upstream_mihomo(raw)))
                .collect(),
        )
    };
    dns_map.insert(serde_yaml::Value::from("nameserver"), to_seq(nameserver));
    if !dns.bootstrap_nameservers.is_empty() {
        dns_map.insert(
            serde_yaml::Value::from("proxy-server-nameserver"),
            to_seq(&dns.bootstrap_nameservers),
        );
    }
    if !dns.direct_nameservers.is_empty() {
        dns_map.insert(
            serde_yaml::Value::from("direct-nameserver"),
            to_seq(&dns.direct_nameservers),
        );
    }
    let mut patch = serde_yaml::Mapping::new();
    patch.insert(
        serde_yaml::Value::from("dns"),
        serde_yaml::Value::Mapping(dns_map),
    );
    let patch_yaml =
        serde_yaml::to_string(&serde_yaml::Value::Mapping(patch)).map_err(|e| e.to_string())?;
    crate::override_patch::apply_merge(&config, &patch_yaml).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_requires_upstream_in_custom_mode() {
        let settings = DnsSettings {
            mode: DnsMode::Custom,
            ..Default::default()
        };
        assert!(validate(&settings).is_err());
    }

    #[test]
    fn inherit_mode_allows_empty() {
        let settings = DnsSettings::default();
        assert!(validate(&settings).is_ok());
    }

    #[test]
    fn parse_accepts_mihomo_style_forms() {
        let https = parse_upstream("https://doh.pub/dns-query").unwrap();
        assert_eq!(https.scheme, "https");
        assert_eq!(https.server, "doh.pub");
        assert_eq!(https.port, 443);
        assert_eq!(https.path.as_deref(), Some("/dns-query"));

        let tls = parse_upstream("tls://dns.google:853").unwrap();
        assert_eq!(tls.port, 853);

        let bare = parse_upstream("223.5.5.5").unwrap();
        assert_eq!(bare.scheme, "udp");
        assert_eq!(bare.server, "223.5.5.5");
        assert_eq!(bare.port, 53);
    }

    #[test]
    fn parse_rejects_unknown_scheme_and_empty_host() {
        assert!(parse_upstream("quic://dns.example").is_err());
        assert!(parse_upstream("https://").is_err());
        assert!(parse_upstream("").is_err());
    }

    #[test]
    fn parse_handles_ipv6_brackets() {
        let v6 = parse_upstream("udp://[::1]:5353").unwrap();
        assert_eq!(v6.server, "::1");
        assert_eq!(v6.port, 5353);
    }

    #[test]
    fn singbox_json_carries_detour_and_path() {
        let upstream = parse_upstream("https://doh.pub/dns-query").unwrap();
        let json = upstream_singbox_json(&upstream, "proxy-dns-0", Some("HK-01"));
        assert_eq!(json["type"], "https");
        assert_eq!(json["server"], "doh.pub");
        assert_eq!(json["path"], "/dns-query");
        assert_eq!(json["detour"], "HK-01");
        assert!(json.get("port").is_none()); // 默认端口省略
    }
}
