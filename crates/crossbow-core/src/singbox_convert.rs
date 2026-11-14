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
    #[error("{0}")]
    InvalidGroup(String),
    #[error("{0}")]
    UnsupportedGroup(String),
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
    /// url-test 容差（毫秒）；仅校验后透传，未配置交给内核默认（#489）。
    #[serde(default)]
    tolerance: Option<u64>,
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

    // 组解析必须严格：过去 unwrap_or_default 会把整段解析失败静默成
    // 「没有组」，生成丢组的配置（#489 同源的静默降级问题）。
    let groups: Vec<ClashGroup> = match doc.get("proxy-groups") {
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|e| ConvertError::Parse(format!("proxy-groups: {e}")))?,
        None => vec![],
    };

    // 已声明的出站 tag = 节点 + 组 + 内建 DIRECT。组成员引用一律按此校验，
    // 不再用「并入 DIRECT」掩盖无效引用（#489：自动组被并入直连候选后，
    // 条件合适时实际流量会从直连出站）。
    let mut declared: std::collections::BTreeSet<String> =
        proxies.iter().map(|p| p.name.clone()).collect();
    for g in &groups {
        if !declared.insert(g.name.clone()) {
            return Err(ConvertError::InvalidGroup(format!(
                "重复的策略组名「{}」",
                g.name
            )));
        }
    }
    declared.insert("DIRECT".into());

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
        outbounds.push(convert_group(g, &declared)?);
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
    // MATCH 规则只有两段（MATCH,策略），取 nth(1)；无 MATCH 回退 DIRECT。
    let final_out = rules
        .iter()
        .rev()
        .find(|r| r.to_lowercase().starts_with("match,"))
        .and_then(|r| r.split(',').nth(1).map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "DIRECT".into());

    // 引用到的规则集 → remote 定义：sing-box 自行下载、缓存进 cache.db 并周期更新。
    // URL 走 gh-proxy 镜像（CN 直连 raw.githubusercontent 不可达）；下载绕行
    // 跟随 MATCH 主出站（DIRECT 时省略 = 直连）。
    let mut seen_sets = std::collections::BTreeSet::new();
    let rule_set_defs: Vec<Value> = route_rules
        .iter()
        .flat_map(|r| {
            r.get("rule_set")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|t| t.as_str().map(String::from))
        .filter(|t| seen_sets.insert(t.clone()))
        .map(|tag| srs_def(&tag, &final_out))
        .collect();

    // ---- 组装 ----
    let bind = if allow_lan { "::" } else { "127.0.0.1" };
    // 代理解析的 DNS 出口跟随 MATCH 主出站；指向 DIRECT 或省略 detour 都等于
    // 直连解析（sing-box 拒绝「detour 到空 direct 出站」）。
    let mut dns_servers = vec![
        json!({ "type": "https", "tag": "proxy-dns", "server": "8.8.8.8" }),
        // 不带 detour：默认直连拨号；显式指向空 DIRECT 出站会被 sing-box 拒绝
        json!({ "type": "udp", "tag": "local-dns", "server": "223.5.5.5" }),
    ];
    if final_out != "DIRECT" {
        dns_servers[0]["detour"] = json!(&final_out);
    }
    let mut cfg = json!({
        "log": { "level": "info", "timestamp": true },
        "dns": {
            "servers": dns_servers,
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
    if !rule_set_defs.is_empty() {
        cfg["route"]["rule_set"] = json!(rule_set_defs);
    }
    Ok(serde_json::to_string_pretty(&cfg)?)
}

/// 规则集 tag → sing-box remote 规则集定义；geoip-*/geosite-* 映射到
/// meta-rules-dat 的 sing 分支，其余 tag 保持 local（不代下载）。
fn srs_def(tag: &str, detour: &str) -> Value {
    let rel = tag
        .strip_prefix("geoip-")
        .map(|c| format!("geo/geoip/{c}.srs"))
        .or_else(|| {
            tag.strip_prefix("geosite-")
                .map(|n| format!("geo/geosite/{n}.srs"))
        });
    let Some(rel) = rel else {
        return json!({"type": "local", "tag": tag, "format": "binary", "path": format!("{tag}.srs")});
    };
    let mut def = json!({
        "type": "remote",
        "tag": tag,
        "format": "binary",
        "url": format!("https://gh-proxy.com/https://raw.githubusercontent.com/MetaCubeX/meta-rules-dat/sing/{rel}"),
        "update_interval": "24h"
    });
    if detour != "DIRECT" {
        def["download_detour"] = json!(detour);
    }
    def
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

fn convert_group(
    g: &ClashGroup,
    declared: &std::collections::BTreeSet<String>,
) -> Result<Value, ConvertError> {
    // #489：保留原组成员与顺序。DIRECT 仅在原组显式引用时保留——
    // 自动选路组（url-test）被并入直连候选后，测速合适时实际流量会
    // 从直连出站，属于未经用户同意的出口变更。
    if g.proxies.is_empty() {
        return Err(ConvertError::InvalidGroup(format!(
            "策略组「{}」没有成员",
            g.name
        )));
    }
    for member in &g.proxies {
        if !declared.contains(member) {
            return Err(ConvertError::InvalidGroup(format!(
                "策略组「{}」引用了不存在的成员「{}」",
                g.name, member
            )));
        }
    }
    let members = g.proxies.clone();
    match g.kind.as_str() {
        "select" => Ok(json!({
            "type": "selector", "tag": g.name,
            "outbounds": members,
            "default": members[0]
        })),
        "url-test" => {
            let mut out = json!({
                "type": "urltest", "tag": g.name,
                "outbounds": members,
                "url": g.url.clone().unwrap_or_else(|| "http://www.gstatic.com/generate_204".into()),
                "interval": format!("{}s", g.interval.unwrap_or(300).max(60))
            });
            // 容差（毫秒）逐项映射；未配置则交给内核默认值。
            if let Some(tolerance) = g.tolerance {
                out["tolerance"] = json!(tolerance);
            }
            Ok(out)
        }
        // fallback / load-balance 在 sing-box 无等价类型：降级 selector 会
        // 静默失去故障转移/负载分配语义（#489）。宁可拒绝转换——用户可以
        // 继续使用 mihomo 或修改源配置，而不是在切换内核时丢语义。
        "fallback" | "load-balance" => Err(ConvertError::UnsupportedGroup(format!(
            "策略组「{}」类型 {}：sing-box 无等价转换，降级会失去自动选路语义；请继续使用 mihomo 或修改源配置",
            g.name, g.kind
        ))),
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
    proxies: [HK-01, JP-Trojan, DIRECT]
  - name: 自动
    type: url-test
    proxies: [HK-01]
    url: http://www.gstatic.com/generate_204
    interval: 300
    tolerance: 120
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

        // 组引用：显式 DIRECT 保留为成员，顺序与源一致
        let sel = obs.iter().find(|o| o["tag"] == "节点选择").unwrap();
        assert_eq!(sel["outbounds"], json!(["HK-01", "JP-Trojan", "DIRECT"]));
        // 自动组：纯代理成员，转换不得追加直连候选（#489）
        let auto = obs.iter().find(|o| o["tag"] == "自动").unwrap();
        assert_eq!(auto["outbounds"], json!(["HK-01"]));
        assert_eq!(auto["tolerance"], 120);

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
        // MATCH,节点选择（两段）→ route.final = 节点选择
        assert_eq!(v["route"]["final"], "节点选择");
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
        assert_eq!(
            v["dns"]["rules"],
            Value::Null,
            "不应生成 outbound:any DNS 规则"
        );
        assert_eq!(v["route"]["default_domain_resolver"], "local-dns");
    }

    // GEOIP/GEOSITE 规则引用的规则集必须带定义；默认 remote（sing-box 自下载），
    // URL 走 gh-proxy 镜像；MATCH 主出站非 DIRECT 时作为 download_detour。
    #[test]
    fn rule_set_refs_get_remote_defs() {
        let out = convert_to_singbox(SAMPLE, 7897, false).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        let defs = v["route"]["rule_set"]
            .as_array()
            .expect("应有 rule_set 定义");
        assert!(defs.iter().any(|d| d["tag"] == "geoip-cn"
            && d["type"] == "remote"
            && d["format"] == "binary"
            && d["url"]
                .as_str()
                .map(|u| u.contains("gh-proxy.com") && u.ends_with("/geo/geoip/cn.srs"))
                .unwrap_or(false)));
        // SAMPLE 的 MATCH → 节点选择（非 DIRECT）→ download_detour 跟随
        assert!(defs[0]
            .get("download_detour")
            .map(|d| d == "节点选择")
            .unwrap_or(false));
        assert_eq!(defs.len(), 1, "SAMPLE 只引用 geoip-cn");
    }

    // MATCH → DIRECT 时：DNS 不带 detour（空 direct 出站会被 sing-box 拒绝），
    // 规则集 download_detour 也省略。
    #[test]
    fn direct_final_omits_detours() {
        let yaml = "proxies:\n  - name: a\n    type: ss\n    server: s\n    port: 1\n    cipher: aes-128-gcm\n    password: p\nrules:\n  - GEOIP,CN,DIRECT\n  - MATCH,DIRECT\n";
        let out = convert_to_singbox(yaml, 7897, false).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["dns"]["servers"][0].get("detour").is_none());
        assert!(v["route"]["rule_set"][0].get("download_detour").is_none());
    }
    // ===== #489：策略组成员与选路语义 =====

    // url-test 自动组不得被追加直连候选；未配置 tolerance 时不出现在产物里。
    #[test]
    fn url_test_group_keeps_proxy_only_members_and_omits_tolerance() {
        let yaml = "proxies:\n  - name: a\n    type: ss\n    server: s\n    port: 1\n    cipher: aes-128-gcm\n    password: p\nproxy-groups:\n  - name: auto\n    type: url-test\n    proxies: [a]\nrules:\n  - MATCH,auto\n";
        let out = convert_to_singbox(yaml, 7897, false).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        let auto = v["outbounds"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["tag"] == "auto")
            .unwrap();
        assert_eq!(auto["type"], "urltest");
        assert_eq!(auto["outbounds"], json!(["a"]));
        assert!(auto.get("tolerance").is_none());
    }

    // 显式引用 DIRECT 的组保留直连成员；规则里的 DIRECT 不受影响。
    #[test]
    fn explicit_direct_member_survives() {
        let yaml = "proxies:\n  - name: a\n    type: ss\n    server: s\n    port: 1\n    cipher: aes-128-gcm\n    password: p\nproxy-groups:\n  - name: manual\n    type: select\n    proxies: [a, DIRECT]\nrules:\n  - MATCH,manual\n";
        let out = convert_to_singbox(yaml, 7897, false).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        let manual = v["outbounds"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["tag"] == "manual")
            .unwrap();
        assert_eq!(manual["outbounds"], json!(["a", "DIRECT"]));
        assert_eq!(v["route"]["final"], "manual");
    }

    // fallback / load-balance 无 sing-box 等价类型：拒绝转换并点名策略组，
    // 不再静默降级成 selector 丢失自动选路语义。
    #[test]
    fn fallback_and_load_balance_fail_with_group_name() {
        for kind in ["fallback", "load-balance"] {
            let yaml = format!(
                "proxies:\n  - name: a\n    type: ss\n    server: s\n    port: 1\n    cipher: aes-128-gcm\n    password: p\nproxy-groups:\n  - name: 分流组\n    type: {kind}\n    proxies: [a]\nrules:\n  - MATCH,分流组\n"
            );
            let err = convert_to_singbox(&yaml, 7897, false).unwrap_err();
            assert!(
                matches!(err, ConvertError::UnsupportedGroup(ref m) if m.contains("分流组") && m.contains(kind)),
                "unexpected: {err}"
            );
        }
    }

    // 空组与未知成员引用：可定位到策略组名，而不是静默并入 DIRECT 兜底。
    #[test]
    fn empty_group_and_unknown_member_fail_locatably() {
        let empty = "proxies:\n  - name: a\n    type: ss\n    server: s\n    port: 1\n    cipher: aes-128-gcm\n    password: p\nproxy-groups:\n  - name: 空组\n    type: select\n    proxies: []\nrules:\n  - MATCH,空组\n";
        let err = convert_to_singbox(empty, 7897, false).unwrap_err();
        assert!(
            matches!(err, ConvertError::InvalidGroup(ref m) if m.contains("空组") && m.contains("没有成员")),
            "unexpected: {err}"
        );

        let unknown = "proxies:\n  - name: a\n    type: ss\n    server: s\n    port: 1\n    cipher: aes-128-gcm\n    password: p\nproxy-groups:\n  - name: 组\n    type: select\n    proxies: [ghost]\nrules:\n  - MATCH,组\n";
        let err = convert_to_singbox(unknown, 7897, false).unwrap_err();
        assert!(
            matches!(err, ConvertError::InvalidGroup(ref m) if m.contains("组」") && m.contains("ghost")),
            "unexpected: {err}"
        );
    }

    // 整段 proxy-groups 解析失败必须报错，不得静默丢组（旧 unwrap_or_default）。
    #[test]
    fn malformed_proxy_groups_fail_loudly() {
        let yaml = "proxies:\n  - name: a\n    type: ss\n    server: s\n    port: 1\n    cipher: aes-128-gcm\n    password: p\nproxy-groups:\n  - name: 组\n    type: select\n    proxies: [a]\n    tolerance: -50\nrules:\n  - MATCH,组\n";
        let err = convert_to_singbox(yaml, 7897, false).unwrap_err();
        assert!(
            matches!(err, ConvertError::Parse(ref m) if m.contains("proxy-groups")),
            "unexpected: {err}"
        );
    }
}
