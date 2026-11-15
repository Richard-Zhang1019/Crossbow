//! Clash YAML → sing-box JSON 配置转换器（M1.3）。
//!
//! 覆盖日常订阅的核心子集：
//! - inbound：mixed 监听（127.0.0.1:port / 0.0.0.0:port）
//! - outbound：ss / vmess / trojan / vless / hysteria2 / http / socks
//! - outbound group：selector / urltest，保留成员、默认选择和测速间隔
//! - route rules：DOMAIN/SUFFIX/KEYWORD、IP-CIDR(-6)、GEOIP、GEOSITE、
//!   DST-PORT、PROCESS-NAME + MATCH → final
//! - dns：国内域名走本地解析，其余经代理解析（防泄漏）
//! - experimental：缓存文件（store-selected 等价）
//!
//! 未覆盖的类型会报错并标明类型名——宁缺勿错，避免静默生成坏配置。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

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
const MIHOMO_DEFAULT_TEST_URL: &str = "https://www.gstatic.com/generate_204";

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
    interval: Option<Value>,
    /// 保留原值，在 url-test 转换时连同组名校验类型、范围和零值语义（#489）。
    #[serde(default)]
    tolerance: Option<Value>,
    /// #500：以下行为字段必须等价映射或显式拒绝，禁止静默丢弃。
    #[serde(default, rename = "include-all-proxies")]
    include_all_proxies: bool,
    /// include-all 含提供者节点，转换器无法等价展开成员集合。
    #[serde(default, rename = "include-all")]
    include_all: bool,
    #[serde(default, rename = "use")]
    use_providers: Vec<String>,
    #[serde(default)]
    filter: Option<String>,
    #[serde(default, rename = "disable-udp")]
    disable_udp: Option<bool>,
    #[serde(default, rename = "expected-status")]
    expected_status: Option<Value>,
    #[serde(default)]
    lazy: Option<bool>,
    #[serde(default, rename = "default-selected")]
    default_selected: Option<Value>,
    #[serde(flatten)]
    other_fields: BTreeMap<String, Value>,
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

    // #508：区分「规则段缺省」「合法空列表」「段类型错误」——后者必须明确
    // 失败（报序号与类型），而不是静默清空整段规则后回退 DIRECT 直连。
    let rules: Vec<String> = match doc.get("rules") {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(items)) => {
            let mut parsed = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                match item.as_str() {
                    Some(rule) if !rule.trim().is_empty() => {
                        parsed.push(rule.trim().to_string());
                    }
                    _ => {
                        return Err(ConvertError::Parse(format!(
                            "rules 第 {} 条不是字符串规则（实际类型 {}）",
                            index + 1,
                            match item {
                                Value::Null => "null",
                                Value::Bool(_) => "bool",
                                Value::Number(_) => "number",
                                Value::Object(_) => "object",
                                _ => "非字符串值",
                            }
                        )));
                    }
                }
            }
            parsed
        }
        Some(other) => {
            return Err(ConvertError::Parse(format!(
                "rules 段类型错误：期望列表，实际 {}",
                match other {
                    Value::Bool(_) => "bool",
                    Value::Number(_) => "number",
                    Value::String(_) => "string",
                    Value::Object(_) => "object",
                    _ => "未知结构",
                }
            )));
        }
    };

    // ---- outbounds ----
    let mut outbounds: Vec<Value> = Vec::new();
    for p in &proxies {
        outbounds.push(convert_proxy(p)?);
    }
    // mihomo v1.19.32 sorts the declared proxy names before expanding group members.
    let mut all_proxy_names: Vec<_> = proxies.iter().map(|p| p.name.clone()).collect();
    all_proxy_names.sort_unstable();
    for g in &groups {
        outbounds.push(convert_group(g, &declared, &all_proxy_names)?);
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

/// #500：行为字段无法等价表达时的可定位拒绝（组名 + 字段 + 值 + 原因）。
fn unsupported_field(g: &ClashGroup, field: &str, value: &str, why: &str) -> ConvertError {
    ConvertError::UnsupportedGroup(format!(
        "策略组「{}」字段 {field} = {value}：sing-box 无等价表达（{why}）；请继续使用 mihomo 或修改源配置",
        g.name
    ))
}

fn convert_group(
    g: &ClashGroup,
    declared: &std::collections::BTreeSet<String>,
    all_proxy_names: &[String],
) -> Result<Value, ConvertError> {
    // #500：成员来源与行为字段先行审计——无法等价表达即拒绝，禁止
    // 静默丢弃后改变可选节点、UDP 策略或健康判定。
    if !g.use_providers.is_empty() {
        return Err(unsupported_field(
            g,
            "use",
            &g.use_providers.join(","),
            "节点提供者不在转换模型内，成员集合无法等价展开",
        ));
    }
    if g.include_all {
        return Err(unsupported_field(
            g,
            "include-all",
            "true",
            "成员来源含提供者节点，转换器无法等价展开",
        ));
    }
    if let Some(filter) = g
        .filter
        .as_deref()
        .filter(|filter| g.include_all_proxies && !filter.is_empty())
    {
        return Err(unsupported_field(
            g,
            "filter",
            filter,
            "成员过滤规则不在转换模型内",
        ));
    }
    if g.disable_udp == Some(true) {
        return Err(unsupported_field(
            g,
            "disable-udp",
            "true",
            "sing-box 策略组无逐组 UDP 开关",
        ));
    }
    if let Some(expected) = g
        .expected_status
        .as_ref()
        .filter(|value| !matches!(value.as_str().map(str::trim), Some("" | "*")))
    {
        return Err(unsupported_field(
            g,
            "expected-status",
            &expected.to_string(),
            "sing-box urltest 无健康状态码判定字段",
        ));
    }
    if g.lazy == Some(false) {
        return Err(unsupported_field(
            g,
            "lazy",
            "false",
            "sing-box urltest 无按需测速开关",
        ));
    }
    // These fields affect provider membership, fallback or health checks in mihomo.
    // Keep the original value for diagnostics and accept explicit no-op defaults.
    for (field, value) in &g.other_fields {
        if value.is_null() {
            continue;
        }
        let reason = match field.as_str() {
            "include-all-providers"
                if value.as_bool() != Some(false) && value.as_u64() != Some(0) =>
            {
                "转换器无法展开节点提供者"
            }
            "exclude-filter" | "exclude-type" if value.as_str() != Some("") => {
                "转换器无法保留成员排除规则"
            }
            "empty-fallback" if !matches!(value.as_str(), Some("" | "COMPATIBLE")) => {
                "sing-box 策略组无空组回退字段"
            }
            "timeout" if value.as_u64() != Some(0) => "sing-box 策略组无逐组测速超时字段",
            "max-failed-times" if value.as_u64() != Some(0) => {
                "sing-box 策略组无失败次数触发重测字段"
            }
            _ => continue,
        };
        return Err(unsupported_field(g, field, &value.to_string(), reason));
    }
    // #489：保留原组成员与顺序。DIRECT 仅在原组显式引用时保留——
    // 自动选路组（url-test）被并入直连候选后，测速合适时实际流量会
    // 从直连出站，属于未经用户同意的出口变更。
    let mut members = g.proxies.clone();
    if g.include_all_proxies {
        // The mihomo parser appends the sorted list after explicit members, including repeats.
        members.extend_from_slice(all_proxy_names);
        if members.is_empty() {
            return Err(unsupported_field(
                g,
                "include-all-proxies",
                "true",
                "没有已声明节点时 mihomo 使用空组回退",
            ));
        }
    }
    if members.is_empty() {
        return Err(ConvertError::InvalidGroup(format!(
            "策略组「{}」没有成员",
            g.name
        )));
    }
    for member in &members {
        if !declared.contains(member) {
            return Err(ConvertError::InvalidGroup(format!(
                "策略组「{}」引用了不存在的成员「{}」",
                g.name, member
            )));
        }
    }
    match g.kind.as_str() {
        "select" => {
            // selector 无健康检查字段：显式 url/interval 会静默丢失。
            if let Some(url) = g
                .url
                .as_deref()
                .filter(|url| !url.is_empty() && *url != MIHOMO_DEFAULT_TEST_URL)
            {
                return Err(unsupported_field(
                    g,
                    "url",
                    url,
                    "sing-box selector 无健康检查字段",
                ));
            }
            if let Some(interval) = g.interval.as_ref().filter(|interval| interval.as_u64() != Some(0)) {
                return Err(unsupported_field(
                    g,
                    "interval",
                    &interval.to_string(),
                    "sing-box selector 无健康检查字段",
                ));
            }
            let default = match &g.default_selected {
                Some(raw) => raw.as_str().ok_or_else(|| {
                    ConvertError::InvalidGroup(format!(
                        "策略组「{}」的 default-selected={raw} 非法：必须是成员名称",
                        g.name
                    ))
                })?,
                None => "",
            };
            // Resolve stale defaults before handing the selector to sing-box.
            let default = members.iter().find(|member| member.as_str() == default).unwrap_or(&members[0]);
            Ok(json!({
                "type": "selector", "tag": g.name,
                "outbounds": members,
                "default": default
            }))
        }
        "url-test" => {
            let tolerance = convert_urltest_tolerance(g)?;
            const MAX_INTERVAL_SECONDS: u64 = i64::MAX as u64 / 1_000_000_000;
            let seconds = match &g.interval {
                None => 0,
                Some(raw) => raw
                    .as_u64()
                    .filter(|seconds| *seconds <= MAX_INTERVAL_SECONDS)
                    .ok_or_else(|| {
                        ConvertError::InvalidGroup(format!(
                            "策略组「{}」的 interval={raw} 非法：必须是 0..{MAX_INTERVAL_SECONDS} 的整数（秒）",
                            g.name
                        ))
                    })?,
            };
            // mihomo uses 300s for a missing or zero interval on static URLTest groups.
            let seconds = if seconds == 0 { 300 } else { seconds };
            let mut outbound = json!({
                "type": "urltest", "tag": g.name,
                "outbounds": members,
                "url": g.url.as_deref().filter(|url| !url.is_empty()).unwrap_or(MIHOMO_DEFAULT_TEST_URL),
                "interval": format!("{seconds}s"),
                "tolerance": tolerance
            });
            // sing-box requires idle_timeout >= interval and defaults it to 30 minutes.
            if seconds > 30 * 60 {
                outbound["idle_timeout"] = json!(format!("{seconds}s"));
            }
            Ok(outbound)
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

fn convert_urltest_tolerance(g: &ClashGroup) -> Result<u16, ConvertError> {
    // mihomo 缺省值为 0；sing-box 1.14.2 会把 0/缺省改成 50ms，不能
    // 通过透传或省略保留零容差。按不支持等价转换处理，禁止静默改选路。
    let tolerance = match &g.tolerance {
        None => 0,
        Some(raw) => raw
            .as_u64()
            .and_then(|value| u16::try_from(value).ok())
            .ok_or_else(|| {
                ConvertError::InvalidGroup(format!(
                    "策略组「{}」的 tolerance={raw} 非法：必须是 0..65535 的整数（毫秒）",
                    g.name
                ))
            })?,
    };
    if tolerance == 0 {
        let source = if g.tolerance.is_some() {
            "tolerance=0"
        } else {
            "tolerance 未配置（mihomo 默认 0ms）"
        };
        return Err(ConvertError::UnsupportedGroup(format!(
            "策略组「{}」的 {source}：sing-box {SINGBOX_VERSION} 会改用 50ms，无法等价保留零容差；请继续使用 mihomo，或在源配置中显式设置 1..65535ms 的正 tolerance",
            g.name
        )));
    }
    Ok(tolerance)
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

    fn url_test_config(tolerance: &str) -> String {
        format!(
            "proxies:\n  - name: a\n    type: ss\n    server: s\n    port: 1\n    cipher: aes-128-gcm\n    password: p\nproxy-groups:\n  - name: auto\n    type: url-test\n    proxies: [a]\n{tolerance}rules:\n  - MATCH,auto\n"
        )
    }

    // 合法边界按值透传，自动组不得被追加直连候选。
    #[test]
    fn url_test_keeps_proxy_only_members_and_valid_tolerance() {
        for tolerance in [1, 120, 65535] {
            let yaml = url_test_config(&format!("    tolerance: {tolerance}\n"));
            let out = convert_to_singbox(&yaml, 7897, false).unwrap();
            let v: Value = serde_json::from_str(&out).unwrap();
            let auto = v["outbounds"]
                .as_array()
                .unwrap()
                .iter()
                .find(|o| o["tag"] == "auto")
                .unwrap();
            assert_eq!(auto["type"], "urltest");
            assert_eq!(auto["outbounds"], json!(["a"]));
            assert_eq!(auto["tolerance"], tolerance);
        }
    }

    #[test]
    fn zero_and_default_tolerance_reject_semantic_change() {
        for tolerance in ["", "    tolerance: 0\n", "    tolerance: null\n"] {
            let err = convert_to_singbox(&url_test_config(tolerance), 7897, false).unwrap_err();
            assert!(
                matches!(err, ConvertError::UnsupportedGroup(ref m)
                    if m.contains("auto") && m.contains("tolerance")
                        && m.contains("0") && m.contains("50ms")
                        && m.contains("mihomo") && m.contains("1..65535")),
                "unexpected: {err}"
            );
        }
    }

    #[test]
    fn invalid_tolerance_reports_group_field_and_value() {
        for tolerance in [
            "-1",
            "65536",
            "18446744073709551615",
            "1.5",
            "\"50\"",
            "true",
        ] {
            let yaml = url_test_config(&format!("    tolerance: {tolerance}\n"));
            let err = convert_to_singbox(&yaml, 7897, false).unwrap_err();
            assert!(
                matches!(err, ConvertError::InvalidGroup(ref m)
                    if m.contains("auto") && m.contains("tolerance")
                        && m.contains(tolerance) && m.contains("0..65535")),
                "unexpected: {err}"
            );
        }
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
        let yaml = "proxies:\n  - name: a\n    type: ss\n    server: s\n    port: 1\n    cipher: aes-128-gcm\n    password: p\nproxy-groups:\n  - name: 组\n    type: select\n    proxies: invalid\nrules:\n  - MATCH,组\n";
        let err = convert_to_singbox(yaml, 7897, false).unwrap_err();
        assert!(
            matches!(err, ConvertError::Parse(ref m) if m.contains("proxy-groups")),
            "unexpected: {err}"
        );
    }

    // ===== #500：策略组行为字段等价映射或可定位拒绝 =====

    /// issue 夹具：include-all-proxies 可展开，disable-udp 必须拒绝。
    #[test]
    fn issue_fixture_disable_udp_fails_locatably() {
        let yaml = "proxies:\n  - name: node-a\n    type: ss\n    server: s\n    port: 1\n    cipher: aes-128-gcm\n    password: p\n  - name: node-b\n    type: ss\n    server: s\n    port: 2\n    cipher: aes-128-gcm\n    password: p\nproxy-groups:\n  - {name: selected, type: select, proxies: [node-a], include-all-proxies: true, disable-udp: true}\n  - {name: auto, type: url-test, proxies: [node-a, node-b], tolerance: 120, interval: 10, expected-status: 204}\nrules:\n  - MATCH,selected\n";
        let err = convert_to_singbox(yaml, 7897, false).unwrap_err();
        assert!(
            matches!(err, ConvertError::UnsupportedGroup(ref m)
                if m.contains("selected") && m.contains("disable-udp")
                    && m.contains("true") && m.contains("UDP")),
            "unexpected: {err}"
        );
    }

    #[test]
    fn selectors_keep_mihomo_membership_order_and_default() {
        let yaml = r#"
proxies:
  - {name: z, type: http, server: 127.0.0.1, port: 8001}
  - {name: a, type: http, server: 127.0.0.1, port: 8002}
proxy-groups:
  - {name: all, type: select, include-all-proxies: true}
  - {name: explicit, type: select, proxies: [z, DIRECT], include-all-proxies: true, default-selected: a}
  - {name: stale, type: select, proxies: [z, a], default-selected: missing}
rules: ['MATCH,all']
"#;
        let out = convert_to_singbox(yaml, 7897, false).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        for (tag, members, default) in [
            ("all", json!(["a", "z"]), "a"),
            ("explicit", json!(["z", "DIRECT", "a", "z"]), "a"),
            ("stale", json!(["z", "a"]), "z"),
        ] {
            let group = v["outbounds"]
                .as_array()
                .unwrap()
                .iter()
                .find(|o| o["tag"] == tag)
                .unwrap();
            assert_eq!(group["outbounds"], members, "{tag}");
            assert_eq!(group["default"], default, "{tag}");
        }
    }

    #[test]
    fn include_all_without_nodes_rejects_implicit_fallback() {
        let yaml = "proxies: []\nproxy-groups:\n  - {name: all, type: select, include-all-proxies: true}\nrules: ['MATCH,all']\n";
        let err = convert_to_singbox(yaml, 7897, false).unwrap_err();
        assert!(
            matches!(err, ConvertError::UnsupportedGroup(ref m)
            if m.contains("all」") && m.contains("include-all-proxies = true")),
            "{err}"
        );
    }

    /// 成员来源与过滤字段：提供者不在转换模型内，一律可定位拒绝。
    #[test]
    fn membership_source_fields_fail_locatably() {
        let base = "proxies:\n  - name: a\n    type: ss\n    server: s\n    port: 1\n    cipher: aes-128-gcm\n    password: p\nproxy-groups:\n  - name: 组\n    type: select\n    proxies: [a]\n{extra}rules:\n  - MATCH,组\n";
        for (extra, field, value) in [
            ("    include-all: true\n", "include-all", "true"),
            (
                "    include-all-providers: true\n",
                "include-all-providers",
                "true",
            ),
            ("    use: [provider-a]\n", "use", "provider-a"),
            (
                "    include-all-proxies: true\n    filter: \".*HK\"\n",
                "filter",
                ".*HK",
            ),
            ("    exclude-filter: HK\n", "exclude-filter", "HK"),
            ("    exclude-type: ss\n", "exclude-type", "ss"),
            ("    empty-fallback: DIRECT\n", "empty-fallback", "DIRECT"),
        ] {
            let yaml = base.replace("{extra}", extra);
            let err = convert_to_singbox(&yaml, 7897, false).unwrap_err();
            assert!(
                matches!(err, ConvertError::UnsupportedGroup(ref m)
                    if m.contains("组」") && m.contains(field) && m.contains(value)),
                "{field}: unexpected: {err}"
            );
        }
    }

    #[test]
    fn health_and_udp_fields_fail_with_value() {
        let base = "proxies:\n  - name: a\n    type: ss\n    server: s\n    port: 1\n    cipher: aes-128-gcm\n    password: p\nproxy-groups:\n  - name: auto\n    type: url-test\n    proxies: [a]\n{extra}rules:\n  - MATCH,auto\n";
        for (extra, field, value) in [
            ("    expected-status: 204\n", "expected-status", "204"),
            ("    disable-udp: true\n", "disable-udp", "true"),
            ("    lazy: false\n", "lazy", "false"),
            ("    timeout: 6000\n", "timeout", "6000"),
            ("    max-failed-times: 1\n", "max-failed-times", "1"),
        ] {
            let yaml = base.replace("{extra}", extra);
            let err = convert_to_singbox(&yaml, 7897, false).unwrap_err();
            assert!(
                matches!(err, ConvertError::UnsupportedGroup(ref m)
                    if m.contains("auto」") && m.contains(field) && m.contains(value)),
                "{field}: unexpected: {err}"
            );
        }
    }

    #[test]
    fn urltest_interval_passthrough_and_mihomo_defaults() {
        let base = "proxies:\n  - name: a\n    type: ss\n    server: s\n    port: 1\n    cipher: aes-128-gcm\n    password: p\nproxy-groups:\n  - name: auto\n    type: url-test\n    proxies: [a]\n    tolerance: 120\n{extra}rules:\n  - MATCH,auto\n";

        for (extra, interval, idle_timeout) in [
            ("    interval: 10\n", "10s", Value::Null),
            ("", "300s", Value::Null),
            ("    interval: 0\n    url: ''\n    expected-status: '*'\n    disable-udp: false\n    lazy: true\n    filter: ''\n    include-all-providers: false\n    exclude-type: ''\n    timeout: 0\n", "300s", Value::Null),
            ("    interval: 3600\n", "3600s", json!("3600s")),
        ] {
            let yaml = base.replace("{extra}", extra);
            let out = convert_to_singbox(&yaml, 7897, false).unwrap();
            let v: Value = serde_json::from_str(&out).unwrap();
            let auto = v["outbounds"].as_array().unwrap().iter().find(|o| o["tag"] == "auto").unwrap();
            assert_eq!(auto["interval"], interval);
            assert_eq!(auto["idle_timeout"], idle_timeout);
            assert_eq!(auto["url"], "https://www.gstatic.com/generate_204");
        }

        for bad_interval in ["-1", "1.5", "9223372037"] {
            let yaml = base.replace("{extra}", &format!("    interval: {bad_interval}\n"));
            let err = convert_to_singbox(&yaml, 7897, false).unwrap_err();
            assert!(
                matches!(err, ConvertError::InvalidGroup(ref m)
                if m.contains("auto」") && m.contains(&format!("interval={bad_interval}"))),
                "{err}"
            );
        }
    }

    /// select 上的 url/interval（健康检查配置）：selector 无对应字段，拒绝。
    #[test]
    fn select_health_fields_fail_locatably() {
        let base = "proxies:\n  - name: a\n    type: ss\n    server: s\n    port: 1\n    cipher: aes-128-gcm\n    password: p\nproxy-groups:\n  - name: manual\n    type: select\n    proxies: [a]\n{extra}rules:\n  - MATCH,manual\n";
        for (extra, field) in [
            ("    url: http://www.gstatic.com/generate_204\n", "url"),
            ("    interval: 300\n", "interval"),
        ] {
            let yaml = base.replace("{extra}", extra);
            let err = convert_to_singbox(&yaml, 7897, false).unwrap_err();
            assert!(
                matches!(err, ConvertError::UnsupportedGroup(ref m)
                    if m.contains("manual」") && m.contains(field)),
                "{field}: unexpected: {err}"
            );
        }
    }

    /// #508：规则段混入非字符串（如覆写脚本误写 false）必须报错并给出
    /// 序号——不得静默清空规则后回退 DIRECT 直连。
    #[test]
    fn non_string_rule_fails_with_index() {
        let base = r#"
proxies:
  - {name: node, type: http, server: 127.0.0.1, port: 9001}
rules:
  - MATCH,node
  - false
"#;
        let err = convert_to_singbox(base, 7897, false).unwrap_err();
        assert!(
            err.to_string().contains("第 2 条") && err.to_string().contains("bool"),
            "错误应指明第 2 条规则与实际类型：{err}"
        );
    }

    /// #508：规则段整体类型错误（字符串而非列表）也明确报错。
    #[test]
    fn rules_section_wrong_type_fails() {
        let base = "rules: MATCH,node\nproxies: []\n";
        let err = convert_to_singbox(base, 7897, false).unwrap_err();
        assert!(err.to_string().contains("rules 段类型错误"), "{err}");
    }

    /// #508：合法形态不回归——缺省、空列表、显式 MATCH,DIRECT 都正常。
    #[test]
    fn legal_rules_forms_still_convert() {
        // 缺省
        let no_rules = "proxies:\n  - {name: n, type: http, server: 127.0.0.1, port: 1}\n";
        let out: serde_json::Value = serde_json::from_str(&convert_to_singbox(no_rules, 7897, false).unwrap()).unwrap();
        assert_eq!(out["route"]["final"], "DIRECT");
        // 空列表
        let empty = "rules: []\nproxies:\n  - {name: n, type: http, server: 127.0.0.1, port: 1}\n";
        let out: serde_json::Value = serde_json::from_str(&convert_to_singbox(empty, 7897, false).unwrap()).unwrap();
        assert_eq!(out["route"]["final"], "DIRECT");
        // 显式 MATCH,DIRECT
        let explicit = "rules:\n  - MATCH,DIRECT\nproxies:\n  - {name: n, type: http, server: 127.0.0.1, port: 1}\n";
        let out: serde_json::Value = serde_json::from_str(&convert_to_singbox(explicit, 7897, false).unwrap()).unwrap();
        assert_eq!(out["route"]["final"], "DIRECT");
    }
}
