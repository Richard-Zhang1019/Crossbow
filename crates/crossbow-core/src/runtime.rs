//! 运行时配置注入：渲染后的订阅配置 + 壳层运行时参数 → 交给内核的最终文件。
//!
//! 运行时参数（端口/控制器/日志级别）始终覆盖订阅内容，避免机场配置里的
//! 端口/控制器设置干扰本地控制。控制器目前走 127.0.0.1 TCP + 随机 secret；
//! uds/pipe 收紧在 M1 与内核版本锁定一起做（DESIGN §3.4）。
//!
//! 两个引擎两种格式：mihomo 走 YAML 合并；sing-box 输入输出都是 JSON，
//! 控制器经 experimental.clash_api（Clash 兼容 API）注入。

use serde_json::json;
use serde_yaml::{Mapping, Value};

use crate::model::Engine;
use crate::override_patch::apply_merge;

/// 壳层决定并传给内核的运行时参数。
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// 引擎（决定注入格式与键名）。
    pub engine: Engine,
    /// HTTP + SOCKS5 混合监听端口。
    pub mixed_port: u16,
    /// 是否允许局域网（false 时绑定 127.0.0.1）。
    pub allow_lan: bool,
    /// External Controller 监听端口（仅 127.0.0.1）。
    pub controller_port: u16,
    /// External Controller 访问密钥。
    pub controller_secret: String,
    /// 内核日志级别：debug / info / warning / error / silent。
    pub log_level: String,
    /// TUN 模式（内核创建 utun 接管全局流量）。
    pub tun_enable: bool,
    /// 物理网络存在原生 IPv6 默认路由。false 时 TUN 不注入 inet6 地址/
    /// 分流路由，dns.ipv6 同步关闭（AAAA 过滤）——DIRECT 的 v6 目的地在
    /// v4-only 网络上必然 no route to host（#136 实测）。未探测的平台
    /// 保持 true（现双栈行为）。
    pub native_ipv6: bool,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            engine: Engine::Mihomo,
            mixed_port: 7897,
            allow_lan: false,
            controller_port: 0, // 0 = 由调用方分配空闲端口
            controller_secret: String::new(),
            log_level: "info".into(),
            tun_enable: false,
            native_ipv6: true,
        }
    }
}

/// 注入运行时参数，返回最终配置字符串。`controller_port == 0` 时跳过控制器注入。
pub fn apply_runtime(base: &str, rt: &RuntimeConfig) -> Result<String, String> {
    match rt.engine {
        Engine::SingBox => apply_runtime_singbox(base, rt),
        Engine::Mihomo => apply_runtime_mihomo(base, rt),
    }
}

fn apply_runtime_mihomo(base_yaml: &str, rt: &RuntimeConfig) -> Result<String, String> {
    let mut patch = Mapping::new();
    patch.insert(Value::from("mixed-port"), Value::from(rt.mixed_port));
    patch.insert(Value::from("allow-lan"), Value::from(rt.allow_lan));
    patch.insert(
        Value::from("bind-address"),
        Value::from(if rt.allow_lan { "*" } else { "127.0.0.1" }),
    );
    patch.insert(Value::from("mode"), Value::from("rule"));
    patch.insert(Value::from("log-level"), Value::from(rt.log_level.as_str()));
    if rt.tun_enable {
        // mihomo 内置 sing-tun：gVisor 网栈 + 自动路由；TUN 必须配 DNS（fake-ip）。
        // 覆盖默认路由时保留本机物理网关，睡眠唤醒或出口断开时可继续管理路由。
        let mut tun = Mapping::new();
        tun.insert(Value::from("enable"), Value::from(true));
        tun.insert(Value::from("stack"), Value::from("gvisor"));
        // Windows：固定 wintun 设备名，共存检测以 Crossbow 排除自家适配器（#81/#82）
        #[cfg(target_os = "windows")]
        tun.insert(Value::from("device"), Value::from("Crossbow"));
        tun.insert(Value::from("auto-route"), Value::from(true));
        tun.insert(Value::from("auto-redirect"), Value::from(true));
        tun.insert(Value::from("auto-detect-interface"), Value::from(true));
        // 显式双栈：mihomo 默认的 inet6 地址存在但 macOS 上 auto-route 不会
        // 自动装 v6 路由（实测 IPv6 被阻断）——补 inet6 地址 + 分流路由，
        // 对齐 sing-box inbound 的双栈行为（#21 实测 / #59）。
        // 仅在物理网络有原生 v6 时注入（#136）：v4-only 网络上 DIRECT 的
        // v6 目的地从物理网卡发不出去（no route to host），吸进隧道只会
        // 白等一轮 + warning 刷屏；走代理的 v6 由远端节点出口，不受影响。
        if rt.native_ipv6 {
            tun.insert(
                Value::from("inet6-address"),
                Value::Sequence(vec![Value::from("fdfe:dcba:9876::1/126")]),
            );
            tun.insert(
                Value::from("inet6-route-address"),
                Value::Sequence(vec![Value::from("::/1"), Value::from("8000::/1")]),
            );
            // v6 侧排除本地段（对齐 v4 的 route-exclude-address）：::/1+8000::/1
            // 覆盖全部 v6 空间，不排除会把链路本地/ULA/组播吸进隧道，
            // 路由器管理页、NAS 等 v6 本地访问会断。
            tun.insert(
                Value::from("inet6-route-exclude-address"),
                Value::Sequence(vec![
                    Value::from("fe80::/10"),
                    Value::from("fc00::/7"),
                    Value::from("ff00::/8"),
                ]),
            );
        }
        // 私网全段豁免（#145）：100.64/10 是 CGNAT/Tailscale tailnet 段，
        // 不排除会被吸进隧道后落到兜底规则走代理，tailnet/内网全部不可达
        tun.insert(
            Value::from("route-exclude-address"),
            Value::Sequence(vec![
                Value::from("192.168.0.0/16"),
                Value::from("100.64.0.0/10"),
                Value::from("10.0.0.0/8"),
                Value::from("172.16.0.0/12"),
            ]),
        );
        // Mihomo defaults macOS dns-mode to disabled, so hijacking port 53
        // alone does not make the system resolver use the TUN. Native mode
        // points system DNS at the TUN address; dns-hijack also catches
        // standard DNS requests arriving from other interfaces.
        tun.insert(Value::from("dns-mode"), Value::from("native"));
        tun.insert(
            Value::from("dns-hijack"),
            Value::Sequence(vec![
                Value::from("any:53"),
                Value::from("tcp://any:53"),
            ]),
        );
        let mut dns = Mapping::new();
        dns.insert(Value::from("enable"), Value::from(true));
        dns.insert(Value::from("enhanced-mode"), Value::from("fake-ip"));
        // AAAA 记录默认被过滤（ipv6: false）——浏览器拿不到 v6 地址，
        // 双栈隧道形同虚设；打开后 AAAA 经 fake-ip/tun 走节点。
        // 与 inet6 路由同门控：无原生 v6 时过滤 AAAA，应用拿不到 v6 地址
        // 连试都不会试（#136）。
        dns.insert(Value::from("ipv6"), Value::from(rt.native_ipv6));
        patch.insert(Value::from("tun"), Value::Mapping(tun));
        patch.insert(Value::from("dns"), Value::Mapping(dns));
        // 域名嗅探（#331 后续）：客户端可能拿到被污染的真实解析（内核 DNS
        // 卡死期间 fallback、或应用自带解析），连接会按污染 IP 直连——证书
        // 不匹配/超时。SNI/Host 嗅探 + override-destination 让 mihomo 从
        // 握手恢复真实域名，按域名走规则并在节点侧干净解析，污染 IP 无害化。
        let mut sniff = Mapping::new();
        sniff.insert(Value::from("enable"), Value::from(true));
        sniff.insert(Value::from("override-destination"), Value::from(true));
        sniff.insert(Value::from("force-dns-mapping"), Value::from(true));
        sniff.insert(Value::from("parse-pure-ip"), Value::from(true));
        let mut sniff_tls = Mapping::new();
        sniff_tls.insert(
            Value::from("ports"),
            Value::Sequence(vec![Value::from(443), Value::from(8443)]),
        );
        let mut sniff_http = Mapping::new();
        sniff_http.insert(
            Value::from("ports"),
            Value::Sequence(vec![Value::from(80), Value::from("8080-8880")]),
        );
        sniff_http.insert(Value::from("override-destination"), Value::from(true));
        let mut sniff_protocols = Mapping::new();
        sniff_protocols.insert(Value::from("TLS"), Value::Mapping(sniff_tls));
        sniff_protocols.insert(Value::from("HTTP"), Value::Mapping(sniff_http));
        sniff.insert(Value::from("sniff"), Value::Mapping(sniff_protocols));
        patch.insert(Value::from("sniffer"), Value::Mapping(sniff));
    }
    // 节点选择持久化：内核侧记住手工选择（等价「固定节点」），重启不丢。
    let mut profile = Mapping::new();
    profile.insert(Value::from("store-selected"), Value::from(true));
    patch.insert(Value::from("profile"), Value::from(profile));
    if rt.controller_port != 0 {
        patch.insert(
            Value::from("external-controller"),
            Value::from(format!("127.0.0.1:{}", rt.controller_port)),
        );
        patch.insert(
            Value::from("secret"),
            Value::from(rt.controller_secret.as_str()),
        );
    }
    let patch_yaml =
        serde_yaml::to_string(&Value::Mapping(patch)).map_err(|e| e.to_string())?;
    let merged = apply_merge(base_yaml, &patch_yaml).map_err(|e| e.to_string())?;
    inject_private_direct_rules(merged)
}

/// 私网直连规则前置（#145）：TUN 捕获场景下，tailnet/内网目的地若被
/// 订阅兜底规则（Match→代理）接走，出口节点没有这些路由，内网必然
/// 不可达。放在 rules 头部保证先于一切订阅规则命中。
const PRIVATE_DIRECT_RULES: [&str; 4] = [
    "IP-CIDR,100.64.0.0/10,DIRECT,no-resolve",
    "IP-CIDR,10.0.0.0/8,DIRECT,no-resolve",
    "IP-CIDR,172.16.0.0/12,DIRECT,no-resolve",
    "IP-CIDR,192.168.0.0/16,DIRECT,no-resolve",
];

fn inject_private_direct_rules(merged: String) -> Result<String, String> {
    let mut root: Value = serde_yaml::from_str(&merged).map_err(|e| e.to_string())?;
    let Some(rules) = root
        .as_mapping_mut()
        .and_then(|m| m.get_mut(Value::from("rules")))
        .and_then(Value::as_sequence_mut)
    else {
        return Ok(merged); // 订阅没有 rules 段则不注入
    };
    for rule in PRIVATE_DIRECT_RULES.iter().rev() {
        rules.insert(0, Value::from(*rule));
    }
    serde_yaml::to_string(&root).map_err(|e| e.to_string())
}

fn apply_runtime_singbox(base_json: &str, rt: &RuntimeConfig) -> Result<String, String> {
    let mut v: serde_json::Value =
        serde_json::from_str(base_json).map_err(|e| format!("sing-box 配置非法 JSON: {e}"))?;

    let bind = if rt.allow_lan { "::" } else { "127.0.0.1" };
    if let Some(inbounds) = v.get_mut("inbounds").and_then(serde_json::Value::as_array_mut) {
        for inb in inbounds.iter_mut() {
            if inb.get("type").and_then(serde_json::Value::as_str) == Some("mixed") {
                inb["listen"] = json!(bind);
                inb["listen_port"] = json!(rt.mixed_port);
            }
        }
    }
    if let Some(log) = v.get_mut("log").filter(|l| serde_json::Value::is_object(l)) {
        log["level"] = json!(sb_log_level(&rt.log_level));
    }
    if rt.controller_port != 0 {
        if !v.get("experimental").map(serde_json::Value::is_object).unwrap_or(false) {
            v["experimental"] = json!({});
        }
        v["experimental"]["clash_api"] = json!({
            "external_controller": format!("127.0.0.1:{}", rt.controller_port),
            "secret": rt.controller_secret,
        });
    }
    if rt.tun_enable {
        // sing-box 原生 tun inbound：auto_route 接管全局路由
        if let Some(inbounds) = v
            .get_mut("inbounds")
            .and_then(serde_json::Value::as_array_mut)
        {
            inbounds.push(json!({
                "type": "tun",
                "tag": "tun-in",
                "address": ["172.19.0.1/30", "fdfe:dcba:9876::1/126"],
                "auto_route": true,
                "strict_route": true,
                "route_exclude_address": [
                    "192.168.0.0/16", "100.64.0.0/10", "10.0.0.0/8", "172.16.0.0/12",
                ],
            }));
        }
        // 域名嗅探（同 mihomo sniffer，#331 后续）：污染解析无害化——
        // sniff action 放规则最前，恢复域名后后续规则按域名匹配。
        // route/rules 缺失时自动创建，保证任何配置都注入。
        if v.get_mut("route").map(serde_json::Value::take).map(|r| r.is_object()).unwrap_or(true) {
            v["route"] = json!({"rules": []});
        }
        if !v["route"]["rules"].is_array() {
            v["route"]["rules"] = json!([]);
        }
        if let Some(rules) = v["route"]["rules"].as_array_mut() {
            rules.insert(0, json!({"action": "sniff"}));
        }
    }
    serde_json::to_string_pretty(&v).map_err(|e| e.to_string())
}

/// mihomo 日志级别 → sing-box（warning→warn，silent→disable）。
fn sb_log_level(level: &str) -> &str {
    match level {
        "warning" => "warn",
        "silent" => "disable",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #136：无原生 v6 时 TUN 不注入 inet6 地址/分流路由，dns.ipv6 关闭；
    /// 有原生 v6（默认）时维持 #59 双栈注入。
    #[test]
    fn tun_inet6_injection_gates_on_native_ipv6() {
        let base = "mixed-port: 7897\nproxies: []\n";
        let rt = |native_ipv6| RuntimeConfig {
            tun_enable: true,
            native_ipv6,
            controller_port: 19090,
            controller_secret: "s3cret".into(),
            ..RuntimeConfig::default()
        };
        let with_v6 = apply_runtime(base, &rt(true)).unwrap();
        assert!(with_v6.contains("inet6-address"));
        assert!(with_v6.contains("inet6-route-address"));
        assert!(with_v6.contains("inet6-route-exclude-address"));
        assert!(with_v6.contains("ipv6: true"));
        let without_v6 = apply_runtime(base, &rt(false)).unwrap();
        assert!(!without_v6.contains("inet6-address"));
        assert!(!without_v6.contains("inet6-route-address"));
        assert!(!without_v6.contains("inet6-route-exclude-address"));
        assert!(without_v6.contains("ipv6: false"));
    }

    /// #145：私网豁免与 DIRECT 规则前置——tailnet/内网目的地不走代理
    #[test]
    fn private_network_exemptions() {
        let base = "mixed-port: 7897\nproxies: []\nrules:\n  - MATCH,DIRECT\n";
        let out = apply_runtime(
            base,
            &RuntimeConfig {
                tun_enable: true,
                controller_port: 19090,
                controller_secret: "s3cret".into(),
                ..RuntimeConfig::default()
            },
        )
        .unwrap();
        assert!(out.contains("100.64.0.0/10"));
        assert!(out.contains("10.0.0.0/8"));
        assert!(out.contains("172.16.0.0/12"));
        // 私网 DIRECT 前置：先于订阅的 MATCH 兜底
        let rules_start = out.find("rules:").unwrap();
        let head = &out[rules_start..];
        let first = head.lines().nth(1).unwrap();
        assert!(first.contains("100.64.0.0/10"), "首条应为 tailnet DIRECT: {first}");
        assert!(first.ends_with("DIRECT,no-resolve"));
        assert!(head.contains("MATCH,DIRECT"), "订阅规则必须保留");
    }

    #[test]
    fn runtime_overrides_subscription_ports() {
        let base = "mixed-port: 12345\nproxies: []\n";
        let out = apply_runtime(
            base,
            &RuntimeConfig {
                mixed_port: 7897,
                allow_lan: false,
                controller_port: 19090,
                controller_secret: "s3cret".into(),
                log_level: "info".into(),
                ..RuntimeConfig::default()
            },
        )
        .unwrap();
        assert!(out.contains("mixed-port: 7897"));
        assert!(out.contains("bind-address: 127.0.0.1"));
        assert!(out.contains("127.0.0.1:19090"));
        assert!(out.contains("s3cret"));
        assert!(out.contains("proxies: []"), "订阅内容必须保留");
    }

    #[test]
    fn allow_lan_binds_all() {
        let out = apply_runtime(
            "proxies: []",
            &RuntimeConfig {
                allow_lan: true,
                ..RuntimeConfig::default()
            },
        )
        .unwrap();
        assert!(out.contains("bind-address: '*'"));
        assert!(out.contains("allow-lan: true"));
    }

    #[test]
    fn controller_port_zero_skips_controller() {
        let out = apply_runtime("proxies: []", &RuntimeConfig::default()).unwrap();
        assert!(!out.contains("external-controller"));
        assert!(!out.contains("secret:"));
    }

    // ---- sing-box：JSON in / JSON out，clash_api 注入 ----

    fn sb_base() -> String {
        // 与 singbox_convert 输出同构的最小 JSON
        r#"{
  "log": { "level": "info", "timestamp": true },
  "dns": { "final": "proxy-dns" },
  "inbounds": [
    { "type": "mixed", "tag": "mixed-in", "listen": "127.0.0.1", "listen_port": 7897 }
  ],
  "outbounds": [{ "type": "direct", "tag": "DIRECT" }],
  "route": { "final": "DIRECT" },
  "experimental": { "cache_file": { "enabled": true } }
}"#
        .into()
    }

    #[test]
    fn singbox_runtime_stays_json_and_injects_clash_api() {
        let out = apply_runtime(
            &sb_base(),
            &RuntimeConfig {
                engine: Engine::SingBox,
                mixed_port: 7899,
                allow_lan: true,
                controller_port: 19090,
                controller_secret: "sb-secret".into(),
                log_level: "warning".into(),
                tun_enable: false,
                native_ipv6: true,
            },
        )
        .unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&out).expect("sing-box 最终配置必须是合法 JSON");
        let inb = &v["inbounds"][0];
        assert_eq!(inb["listen_port"], 7899);
        assert_eq!(inb["listen"], "::");
        assert_eq!(v["log"]["level"], "warn");
        assert_eq!(v["experimental"]["clash_api"]["external_controller"], "127.0.0.1:19090");
        assert_eq!(v["experimental"]["clash_api"]["secret"], "sb-secret");
        // 已有 experimental 键不被覆盖
        assert_eq!(v["experimental"]["cache_file"]["enabled"], true);
    }

    #[test]
    fn singbox_controller_zero_skips_clash_api() {
        let out = apply_runtime(
            &sb_base(),
            &RuntimeConfig {
                engine: Engine::SingBox,
                ..RuntimeConfig::default()
            },
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(v["experimental"].get("clash_api").is_none());
        assert_eq!(v["inbounds"][0]["listen_port"], 7897);
    }

    #[test]
    fn singbox_rejects_non_json_base() {
        let err = apply_runtime(
            "mixed-port: 7897\nproxies: []\n",
            &RuntimeConfig {
                engine: Engine::SingBox,
                ..RuntimeConfig::default()
            },
        )
        .unwrap_err();
        assert!(err.contains("JSON"));
    }
}

#[cfg(test)]
mod tun_tests {
    use super::*;

    #[test]
    fn mihomo_tun_injection() {
        let out = apply_runtime(
            "proxies: []",
            &RuntimeConfig {
                tun_enable: true,
                ..RuntimeConfig::default()
            },
        )
        .unwrap();
        assert!(out.contains("tun:"), "应注入 tun 段");
        assert!(out.contains("stack: gvisor"));
        assert!(out.contains("auto-route: true"));
        assert!(out.contains("auto-redirect: true"));
        assert!(out.contains("route-exclude-address:"));
        assert!(out.contains("192.168.0.0/16"));
        assert!(out.contains("dns-mode: native"));
        assert!(out.contains("dns-hijack:"));
        assert!(out.contains("tcp://any:53"));
        assert!(out.contains("enhanced-mode: fake-ip"), "TUN 需 DNS 劫持");
        // 域名嗅探（#331 后续）：污染解析无害化
        assert!(out.contains("sniffer:"), "TUN 需注入 sniffer 段");
        assert!(out.contains("parse-pure-ip: true"));
        assert!(out.contains("override-destination: true"));
        // 双栈（#59）：inet6 地址 + v6 分流路由 + DNS 放行 AAAA
        assert!(out.contains("fdfe:dcba:9876::1/126"));
        assert!(out.contains("inet6-route-address:"));
        assert!(out.contains("::/1"));
        assert!(out.contains("8000::/1"));
        assert!(out.contains("inet6-route-exclude-address:"));
        assert!(out.contains("fe80::/10"));
        assert!(out.contains("ipv6: true"));
    }

    #[test]
    fn mihomo_no_tun_by_default() {
        let out = apply_runtime("proxies: []", &RuntimeConfig::default()).unwrap();
        assert!(!out.contains("tun:"));
    }

    #[test]
    fn singbox_tun_injection() {
        let base = r#"{
            "inbounds": [{ "type": "mixed", "tag": "mixed-in", "listen": "127.0.0.1", "listen_port": 7897 }],
            "outbounds": [{ "type": "direct", "tag": "DIRECT" }]
        }"#;
        let out = apply_runtime(
            base,
            &RuntimeConfig {
                engine: crate::Engine::SingBox,
                tun_enable: true,
                ..RuntimeConfig::default()
            },
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let inb = v["inbounds"].as_array().unwrap();
        assert!(
            inb.iter().any(|i| i["type"] == "tun" && i["auto_route"] == true),
            "应追加 tun inbound"
        );
        // 域名嗅探（#331 后续）：sniff action 在规则最前
        let rules = v["route"]["rules"].as_array().unwrap();
        assert_eq!(rules[0], serde_json::json!({"action": "sniff"}));
    }
}
