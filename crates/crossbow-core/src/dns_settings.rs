//! #598 DNS 配置：中立意图（沿用订阅 / 受控自定义）+ 引擎映射。
//!
//! 自定义上游覆盖覆写/订阅的同名字段；保留 hosts、域名专用策略和 fallback。
//! TUN 运行时最后合并 enable/mode/ipv6，保留本模块设置的上游。
//! Inherit 零改动；不把 nameserver 等同于“绝不泄漏”的保证。
//!
//! mihomo：默认查询显式经 MATCH 出站，直连与节点引导显式 DIRECT。
//! sing-box：Clash 转换路径每类一条上游，proxy-dns 经 MATCH，local-dns
//! 同时负责直连拨号与节点/DNS 服务域名引导，后者须用 IP 防止循环依赖。
//! 不支持的独立引导、多上游及原生 JSON 应用级自定义均明确拒绝。

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
    /// 默认查询上游（跟随 MATCH 出站；订阅专用策略仍保留）
    #[serde(default)]
    pub proxy_nameservers: Vec<String>,
    /// 直连业务上游（直连出站/本地域名）
    #[serde(default)]
    pub direct_nameservers: Vec<String>,
    /// 独立节点引导上游（仅 mihomo；留空时使用直连上游）
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
        if list.len() > 8 {
            return Err("每类 DNS 上游最多 8 条".into());
        }
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
        Some(_) => return Err("不支持的 DNS 上游协议（支持 https/tls/udp）".into()),
        None => ("udp", s),
    };
    // A controlled common subset: never let one engine silently discard URL parts.
    if s.len() > 2048 || s.chars().any(char::is_whitespace) || rest.contains(['#', '?', '@']) {
        return Err("DNS 上游包含不支持的空白、凭据、查询参数或片段".into());
    }
    let url = url::Url::parse(&format!("{scheme}://{rest}"))
        .map_err(|_| "DNS 上游地址或端口无效".to_string())?;
    let server = match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.to_string(),
        Some(url::Host::Ipv6(ip)) => ip.to_string(),
        Some(url::Host::Domain(host)) if !host.is_empty() && !host.contains(':') => {
            host.to_string()
        }
        _ => return Err("DNS 上游缺少有效服务器地址（IPv6 请使用方括号）".into()),
    };
    let port = url.port().unwrap_or_else(|| Upstream::default_port(scheme));
    if port == 0 {
        return Err("DNS 上游端口必须在 1–65535 范围内".into());
    }
    let path = match url.path() {
        "" => None,
        _ if scheme == "https" && !rest.contains('/') => None,
        path if scheme == "https" => Some(path.to_string()),
        _ => return Err("仅 HTTPS DNS 支持路径".into()),
    };
    Ok(Upstream {
        scheme,
        server,
        port,
        path,
    })
}

/// Reject unsupported intent before any persistence, including stopped-core edits.
pub fn validate_for_engine(settings: &DnsSettings, engine: crate::Engine) -> Result<(), String> {
    validate(settings)?;
    if settings.mode == DnsMode::Custom
        && engine == crate::Engine::Mihomo
        && !settings.proxy_nameservers.is_empty()
        && settings.direct_nameservers.is_empty()
        && settings.bootstrap_nameservers.is_empty()
    {
        return Err("代理 DNS 需要直连或节点引导上游，避免循环依赖".into());
    }
    if settings.mode == DnsMode::Custom && engine == crate::Engine::SingBox {
        if !settings.bootstrap_nameservers.is_empty() {
            return Err("sing-box 暂不支持独立节点引导上游；请先清空该项或沿用订阅".into());
        }
        if settings.proxy_nameservers.len() != 1 || settings.direct_nameservers.len() != 1 {
            return Err("sing-box 自定义 DNS 需要代理和直连上游各一条，暂不支持多上游".into());
        }
        let direct = parse_upstream(&settings.direct_nameservers[0])?;
        if direct.server.parse::<std::net::IpAddr>().is_err() {
            return Err("sing-box 直连 DNS 必须使用 IP 地址，避免解析上游自身的循环依赖".into());
        }
    }
    Ok(())
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
        server["server_port"] = serde_json::json!(upstream.port);
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
    validate_for_engine(dns, crate::Engine::Mihomo)?;
    // nameserver（默认解析）：代理业务优先，未填则落到直连上游
    let nameserver = if dns.proxy_nameservers.is_empty() {
        &dns.direct_nameservers
    } else {
        &dns.proxy_nameservers
    };
    let document: serde_yaml::Value =
        serde_yaml::from_str(&config).map_err(|_| "DNS 配置语法无效".to_string())?;
    let target = document
        .get("rules")
        .and_then(serde_yaml::Value::as_sequence)
        .and_then(|rules| {
            rules
                .iter()
                .filter_map(serde_yaml::Value::as_str)
                .find_map(|rule| {
                    rule.strip_prefix("MATCH,")
                        .or_else(|| rule.strip_prefix("FINAL,"))
                })
        })
        .unwrap_or("DIRECT")
        .trim();
    if target.is_empty() || target.contains(['#', '&', ',', '\n', '\r']) {
        return Err("无法为 DNS 确定有效的 MATCH 出站策略".into());
    }
    let mut dns_map = serde_yaml::Mapping::new();
    let to_seq = |list: &[String]| {
        serde_yaml::Value::Sequence(
            list.iter()
                .map(|raw| serde_yaml::Value::from(upstream_mihomo(raw)))
                .collect(),
        )
    };
    let via = |list: &[String], target: &str| -> serde_yaml::Value {
        to_seq(
            &list
                .iter()
                .map(|raw| format!("{}#{target}", raw.trim()))
                .collect::<Vec<_>>(),
        )
    };
    dns_map.insert(
        serde_yaml::Value::from("enable"),
        serde_yaml::Value::Bool(true),
    );
    dns_map.insert(
        serde_yaml::Value::from("nameserver"),
        via(
            nameserver,
            if dns.proxy_nameservers.is_empty() {
                "DIRECT"
            } else {
                target
            },
        ),
    );
    if dns.bootstrap_nameservers.is_empty() && !dns.direct_nameservers.is_empty() {
        dns_map.insert(
            serde_yaml::Value::from("proxy-server-nameserver"),
            via(&dns.direct_nameservers, "DIRECT"),
        );
    } else if !dns.bootstrap_nameservers.is_empty() {
        dns_map.insert(
            serde_yaml::Value::from("proxy-server-nameserver"),
            via(&dns.bootstrap_nameservers, "DIRECT"),
        );
    }
    if !dns.direct_nameservers.is_empty() {
        dns_map.insert(
            serde_yaml::Value::from("direct-nameserver"),
            via(&dns.direct_nameservers, "DIRECT"),
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
        assert!(json.get("server_port").is_none()); // 默认端口省略
    }
}
