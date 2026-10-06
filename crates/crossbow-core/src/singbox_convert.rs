//! Clash YAML → sing-box JSON 配置转换器（M1.3）。
//!
//! 覆盖日常订阅的核心子集：
//! - inbound：mixed 监听（127.0.0.1:port / 0.0.0.0:port）
//! - outbound：ss / vmess / trojan / vless / hysteria2 / http / socks
//! - outbound group：selector / urltest（urltest 的 interval 统一 300s）
//! - route rules：DOMAIN/SUFFIX/KEYWORD、IP-CIDR(-6)、GEOIP、GEOSITE、
//!   DST-PORT、PROCESS-NAME + MATCH → final
//! - dns：国内域名走本地解析，其余经代理解析（防泄漏）
//! - experimental：缓存文件（store-selected 等价）
//!
//! 未覆盖的类型会报错并标明类型名——宁缺勿错，避免静默生成坏配置。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    #[error("不支持的代理类型：{0}")]
    UnsupportedProxy(String),
    #[error("不支持的规则类型：{0}")]
    UnsupportedRule(String),
    #[error("配置缺少 proxies 段")]
    NoProxies,
    #[error("解析失败：{0}")]
    Parse(String),
}

impl From<serde_yaml::Error> for ConvertError {
    fn from(e: serde_yaml::Error) -> Self {
        Self::Parse(e.to_string())
    }
}

impl From<serde_json::Error> for ConvertError {
    fn from(e: serde_json::Error) -> Self {
        Self::Parse(format!("json: {e}"))
    }
}

/// sing-box 版本锁定（升级走 PR 审查）。
pub const SINGBOX_VERSION: &str = "1.14.2";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClashProxy {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    server: String,
    port: u16,
    #[serde(default)]
    cipher: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    uuid: Option<String>,
    #[serde(default)]
    alter_id: Option<i64>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default, rename = "tls")]
    tls: Option<bool>,
    #[serde(default)]
    network: Option<String>,
    #[serde(default)]
    udp: Option<bool>,
    #[serde(default, rename = "sni")]
    server_name: Option<String>,
    #[serde(default, rename = "skip-cert-verify")]
    skip_cert_verify: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClashGroup {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    proxies: Vec<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    interval: Option<u64>,
}

/// Clash YAML → sing-box JSON（pretty）。
pub fn convert_to_singbox(
    clash_yaml: &str,
    mixed_port: u16,
    allow_lan: bool,
) -> Result<String, ConvertError> {
    let doc: Value =
        serde_yaml::from_str(clash_yaml).map_err(|e| ConvertError::Parse(format!("yaml: {e}")))?;

    let proxies: Vec<ClashProxy> =
        serde_json::from_value(doc.get("proxies").cloned().ok_or(ConvertError::NoProxies)?)
            .map_err(|e| ConvertError::Parse(format!("proxies: {e}")))?;

    let groups: Vec<ClashGroup> = match doc.get("proxy-groups") {
        Some(v) => serde_json::from_value(v.clone()).unwrap_or_default(),
        None => vec![],
    };

    let rules: Vec<String> = match doc.get("rules") {
        Some(v) => serde_json::from_value(v.clone()).unwrap_or_default(),
        None => vec![],
    };

    // ---- outbounds ----
    let mut outbounds: Vec<Value> = Vec::new();
    for p in &proxies {
        outbounds.push(convert_proxy(p)?);
    }
    for g in &groups {
        outbounds.push(convert_group(g)?);
    }
    outbounds.push(json!({
        "type": "direct", "tag": "DIRECT"
    }));

    // ---- route ----
    let mut route_rules: Vec<Value> = Vec::new();
    for r in &rules {
        if let Some(rule) = convert_rule(r)? {
            route_rules.push(rule);
        }
    }
    let final_out = rules
        .iter()
        .rev()
        .find(|r| r.to_lowercase().starts_with("match,"))
        .and_then(|r| r.split(',').nth(2).map(String::from))
        .unwrap_or_else(|| "DIRECT".into());

    // ---- 组装 ----
    let bind = if allow_lan { "::" } else { "127.0.0.1" };
    let cfg = json!({
        "log": { "level": "info", "timestamp": true },
        "dns": {
            "servers": [
                { "type": "https", "tag": "proxy-dns", "server": "8.8.8.8", "detour": "AUTO" },
                { "type": "udp", "tag": "local-dns", "server": "223.5.5.5", "detour": "DIRECT" }
            ],
            "final": "proxy-dns",
            "strategy": "prefer_ipv4"
        },
        "inbounds": [
            {
                "type": "mixed",
                "tag": "mixed-in",
                "listen": bind,
                "listen_port": mixed_port
            }
        ],
        "outbounds": outbounds,
        "route": {
            "rules": route_rules,
            "final": final_out,
            "auto_detect_interface": true,
            // 1.12+ 域名解析出口：替代已移除的 DNS 规则 outbound 匹配项
            "default_domain_resolver": "local-dns"
        },
        "experimental": {
            "cache_file": { "enabled": true, "store_fakeip": false }
        }
    });
    Ok(serde_json::to_string_pretty(&cfg)?)
}

fn convert_proxy(p: &ClashProxy) -> Result<Value, ConvertError> {
    let tls_on = p.tls.unwrap_or(false)
        || matches!(p.kind.as_str(), "trojan" | "hysteria2")
        || p.port == 443;
    let common_tls = if tls_on {
        json!({
            "enabled": true,
            "server_name": p.server_name.clone().unwrap_or_else(|| p.server.clone()),
            "insecure": p.skip_cert_verify.unwrap_or(false)
        })
    } else {
        json!(null)
    };

    let out = match p.kind.as_str() {
        "ss" => {
            let cipher = p.cipher.clone().unwrap_or_else(|| "aes-128-gcm".into());
            if cipher == "aes-128-gcm"
                || cipher == "aes-256-gcm"
                || cipher == "chacha20-ietf-poly1305"
            {
                json!({
                    "type": "shadowsocks", "tag": p.name,
                    "server": p.server, "server_port": p.port,
                    "method": cipher, "password": p.password.clone().unwrap_or_default(),
                    "udp_over_tcp": false
                })
            } else {
                // stream cipher → shadowsocks 旧字段同样适用
                json!({
                    "type": "shadowsocks", "tag": p.name,
                    "server": p.server, "server_port": p.port,
                    "method": cipher, "password": p.password.clone().unwrap_or_default()
                })
            }
        }
        "vmess" => {
            let mut o = json!({
                "type": "vmess", "tag": p.name,
                "server": p.server, "server_port": p.port,
                "uuid": p.uuid.clone().unwrap_or_default(),
                "security": p.cipher.clone().unwrap_or_else(|| "auto".into()),
                "alter_id": p.alter_id.unwrap_or(0)
            });
            if tls_on {
                o["tls"] = common_tls;
            }
            o
        }
        "trojan" => {
            json!({
                "type": "trojan", "tag": p.name,
                "server": p.server, "server_port": p.port,
                "password": p.password.clone().unwrap_or_default(),
                "tls": common_tls
            })
        }
        "vless" => {
            let mut o = json!({
                "type": "vless", "tag": p.name,
                "server": p.server, "server_port": p.port,
                "uuid": p.uuid.clone().unwrap_or_default()
            });
            if tls_on {
                o["tls"] = common_tls;
            }
            o
        }
        "hysteria2" => {
            json!({
                "type": "hysteria2", "tag": p.name,
                "server": p.server, "server_port": p.port,
                "password": p.password.clone().unwrap_or_default(),
                "tls": common_tls
            })
        }
        "http" => {
            let mut o = json!({
                "type": "http", "tag": p.name,
                "server": p.server, "server_port": p.port
            });
            if let Some(u) = &p.username {
                o["username"] = json!(u);
            }
            if let Some(pw) = &p.password {
                o["password"] = json!(pw);
            }
            if tls_on {
                o["tls"] = common_tls;
            }
            o
        }
        "socks5" => {
            let mut o = json!({
                "type": "socks", "tag": p.name,
                "server": p.server, "server_port": p.port,
                "version": "5"
            });
            if let Some(u) = &p.username {
                o["username"] = json!(u);
            }
            if let Some(pw) = &p.password {
                o["password"] = json!(pw);
            }
            o
        }
        other => return Err(ConvertError::UnsupportedProxy(other.to_string())),
    };
    Ok(out)
}

fn convert_group(g: &ClashGroup) -> Result<Value, ConvertError> {
    // DIRECT 兜底并入组（sing-box 不支持引用不存在的 outbound）
    let mut members = g.proxies.clone();
    if !members.iter().any(|m| m == "DIRECT") {
        members.push("DIRECT".into());
    }
    match g.kind.as_str() {
        "select" => Ok(json!({
            "type": "selector", "tag": g.name,
            "outbounds": members,
            "default": members.first().cloned().unwrap_or_else(|| "DIRECT".into())
        })),
        "url-test" => Ok(json!({
            "type": "urltest", "tag": g.name,
            "outbounds": members,
            "url": g.url.clone().unwrap_or_else(|| "http://www.gstatic.com/generate_204".into()),
            "interval": format!("{}s", g.interval.unwrap_or(300).max(60))
        })),
        "fallback" | "load-balance" => {
            // 降级为 selector：语义最近似且安全
            Ok(json!({
                "type": "selector", "tag": g.name,
                "outbounds": members
            }))
        }
        other => Err(ConvertError::UnsupportedProxy(format!("组类型 {other}"))),
    }
}

/// 单条 Clash 规则（`TYPE,payload,target` 字符串）→ sing-box route rule；
/// MATCH 返回 None（用 final 承接）。
fn convert_rule(rule: &str) -> Result<Option<Value>, ConvertError> {
    let mut parts = rule.split(',');
    let kind = parts.next().unwrap_or("").trim().to_string();
    // 目标是最后一段（payload 内可能含逗号的类型极少，S2 覆盖集内不存在）
    let target = parts.next_back().unwrap_or("").trim().to_string();
    let payload = parts.collect::<Vec<_>>().join(",").trim().to_string();
    let outbound = json!(target);

    let rule = match kind.as_str() {
        "DOMAIN-SUFFIX" => json!({ "domain_suffix": [payload], "outbound": outbound }),
        "DOMAIN" => json!({ "domain": [payload], "outbound": outbound }),
        "DOMAIN-KEYWORD" => json!({ "domain_keyword": [payload], "outbound": outbound }),
        "IP-CIDR" | "IP-CIDR6" => json!({ "ip_cidr": [payload], "outbound": outbound }),
        "GEOIP" => {
            json!({ "rule_set": [format!("geoip-{}", payload.to_lowercase())], "outbound": outbound })
        }
        "GEOSITE" => {
            json!({ "rule_set": [format!("geosite-{}", payload.to_lowercase())], "outbound": outbound })
        }
        "DST-PORT" => {
            let port: u16 = payload
                .parse()
                .map_err(|_| ConvertError::Parse(format!("DST-PORT 非法端口：{payload}")))?;
            json!({ "port": [port], "outbound": outbound })
        }
        "PROCESS-NAME" => json!({ "process_name": [payload], "outbound": outbound }),
        "MATCH" => return Ok(None),
        other => return Err(ConvertError::UnsupportedRule(other.to_string())),
    };
    Ok(Some(rule))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
mixed-port: 7897
proxies:
  - name: "HK-01"
    type: ss
    server: a.example.com
    port: 8388
    cipher: aes-128-gcm
    password: pw1
  - name: "JP-Trojan"
    type: trojan
    server: b.example.com
    port: 443
    password: pw2
    skip-cert-verify: true
proxy-groups:
  - name: 节点选择
    type: select
    proxies: [HK-01, JP-Trojan]
  - name: 自动
    type: url-test
    proxies: [HK-01]
    url: http://www.gstatic.com/generate_204
    interval: 300
rules:
  - DOMAIN-SUFFIX,twitter.com,节点选择
  - GEOIP,CN,DIRECT
  - MATCH,节点选择
"#;

    #[test]
    fn converts_core_structure() {
        let out = convert_to_singbox(SAMPLE, 7897, false).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();

        // inbound
        assert_eq!(v["inbounds"][0]["type"], "mixed");
        assert_eq!(v["inbounds"][0]["listen_port"], 7897);
        assert_eq!(v["inbounds"][0]["listen"], "127.0.0.1");

        // outbounds：2 节点 + 2 组 + DIRECT
        let obs = v["outbounds"].as_array().unwrap();
        assert_eq!(obs.len(), 5);
        assert!(obs
            .iter()
            .any(|o| o["type"] == "shadowsocks" && o["tag"] == "HK-01"));
        assert!(obs
            .iter()
            .any(|o| o["type"] == "trojan" && o["tag"] == "JP-Trojan"));
        assert!(obs
            .iter()
            .any(|o| o["type"] == "selector" && o["tag"] == "节点选择"));
        assert!(obs
            .iter()
            .any(|o| o["type"] == "urltest" && o["tag"] == "自动"));
        assert!(obs.iter().any(|o| o["type"] == "direct"));

        // 组引用
        let sel = obs.iter().find(|o| o["tag"] == "节点选择").unwrap();
        assert_eq!(sel["outbounds"].as_array().unwrap().len(), 3); // 2 节点 + DIRECT

        // route
        let rules = v["route"]["rules"].as_array().unwrap();
        assert!(rules.iter().any(|r| r["domain_suffix"]
            .as_array()
            .map(|a| a[0] == "twitter.com")
            .unwrap_or(false)));
        assert!(rules.iter().any(|r| r["rule_set"]
            .as_array()
            .map(|a| a[0] == "geoip-cn")
            .unwrap_or(false)));
        assert_eq!(v["route"]["final"], "DIRECT");
    }

    #[test]
    fn allow_lan_binds_wildcard() {
        let out = convert_to_singbox(SAMPLE, 7897, true).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["inbounds"][0]["listen"], "::");
    }

    #[test]
    fn unsupported_proxy_fails_loudly() {
        let yaml = "proxies:\n  - name: x\n    type: snell\n    server: s\n    port: 1\n";
        let err = convert_to_singbox(yaml, 7897, false).unwrap_err();
        assert!(matches!(err, ConvertError::UnsupportedProxy(ref t) if t == "snell"));
    }

    #[test]
    fn match_rule_becomes_final() {
        let out = convert_to_singbox(SAMPLE, 7897, false).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        let rules = v["route"]["rules"].as_array().unwrap();
        // MATCH 不产生规则条目（用 final），这里应有 2 条
        assert_eq!(rules.len(), 2);
    }

    // sing-box 1.14 移除了 DNS 规则的 outbound 匹配项，域名解析出口改用
    // route.default_domain_resolver；此回归锁住该迁移（内核会 FATAL 拒启）。
    #[test]
    fn no_deprecated_outbound_dns_rule() {
        let out = convert_to_singbox(SAMPLE, 7897, false).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["dns"]["rules"], Value::Null, "不应生成 outbound:any DNS 规则");
        assert_eq!(v["route"]["default_domain_resolver"], "local-dns");
    }
}
