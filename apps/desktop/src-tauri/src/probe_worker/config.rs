//! Reduce an already-running config to the chosen concrete outbound and its dialer chain.
//! Never copy inbounds, TUN, DNS listeners, providers, groups, rule sets or download tasks.
use serde_json::{json, Value};
use std::collections::HashSet;

pub(crate) struct Config {
    pub document: Value,
    pub udp_supported: bool,
}

pub(crate) fn build(
    text: &str,
    engine: crossbow_core::Engine,
    node: &str,
    port: u16,
    password: &str,
) -> Result<Config, String> {
    let source: Value = serde_yaml::from_str(text).map_err(|_| "无法解析当前内核运行配置")?;
    let (key, name, dependency) = if engine == crossbow_core::Engine::Mihomo {
        ("proxies", "name", "dialer-proxy")
    } else {
        ("outbounds", "tag", "detour")
    };
    let items = source
        .get(key)
        .and_then(Value::as_array)
        .ok_or("当前配置没有可隔离测试的内联节点")?;
    let mut selected = Vec::new();
    let mut visited = HashSet::new();
    fn collect(
        items: &[Value],
        key: &str,
        dependency: &str,
        node: &str,
        selected: &mut Vec<Value>,
        visited: &mut HashSet<String>,
    ) -> Result<(), String> {
        if visited.len() >= 16 || !visited.insert(node.into()) {
            return Err("节点拨号链循环或过长，无法隔离测试".into());
        }
        let item = items
            .iter()
            .find(|v| v.get(key).and_then(Value::as_str) == Some(node))
            .ok_or("此节点来自远程 provider 或策略组，暂不支持隔离分项测试；请选择内联具体节点")?;
        let kind = item.get("type").and_then(Value::as_str).unwrap_or("");
        // Explicit allowlist excludes protocols that can alter host routing or start persistent background work.
        if !matches!(
            kind,
            "ss" | "ssr"
                | "shadowsocks"
                | "shadowsocksr"
                | "vmess"
                | "vless"
                | "trojan"
                | "hysteria"
                | "hysteria2"
                | "tuic"
                | "http"
                | "socks"
                | "socks5"
                | "anytls"
                | "direct"
                | "snell"
        ) {
            return Err("此节点类型不支持隔离分项测试，请选择普通代理节点".into());
        }
        for field in [
            "client-fingerprint",
            "certificate",
            "private-key",
            "certificate_path",
            "key_path",
        ] {
            if item.get(field).is_some_and(|v| {
                v.is_string()
                    && (v.as_str().unwrap_or("").contains('/')
                        || v.as_str().unwrap_or("").contains('\\'))
            }) && field != "client-fingerprint"
            {
                return Err("节点依赖外部证书文件，暂不支持隔离分项测试".into());
            }
        }
        if let Some(next) = item.get(dependency).and_then(Value::as_str) {
            collect(items, key, dependency, next, selected, visited)?;
        }
        selected.push(item.clone());
        Ok(())
    }
    collect(items, name, dependency, node, &mut selected, &mut visited)?;
    let chosen = selected.last().ok_or("没有可测试节点")?;
    let kind = chosen["type"].as_str().unwrap_or("");
    let udp_supported = kind != "http"
        && chosen.get("udp").and_then(Value::as_bool) != Some(false)
        && chosen.get("network").and_then(Value::as_str) != Some("tcp");
    let document = if engine == crossbow_core::Engine::Mihomo {
        let group = format!("cb-probe-{}", uuid::Uuid::new_v4().simple());
        json!({"mixed-port":port,"allow-lan":false,"bind-address":"127.0.0.1","authentication":[format!("probe:{password}")],"skip-auth-prefixes":[],"mode":"rule","log-level":"silent","ipv6":false,"tun":{"enable":false},"dns":{"enable":false},"profile":{"store-selected":false,"store-fake-ip":false},"proxies":selected,"proxy-groups":[{"name":group,"type":"select","proxies":[node]}],"rules":[format!("MATCH,{group}")]})
    } else {
        json!({"log":{"disabled":true},"inbounds":[{"type":"mixed","tag":"probe-in","listen":"127.0.0.1","listen_port":port,"users":[{"username":"probe","password":password}]}],"outbounds":selected,"route":{"final":node,"default_domain_resolver":"probe-dns"},"dns":{"servers":[{"type":"local","tag":"probe-dns"}]}})
    };
    Ok(Config {
        document,
        udp_supported,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strips_host_mutations_and_remote_background_work() {
        let source = json!({"mixed-port":7890,"tun":{"enable":true},"listeners":[{"type":"tun"}],"proxy-providers":{"remote":{"url":"https://private.invalid"}},"dns":{"listen":"0.0.0.0:53"},"proxies":[{"name":"a","type":"ss","server":"secret.invalid","password":"secret","dialer-proxy":"b"},{"name":"b","type":"socks5","server":"localhost"},{"name":"unused","type":"ss"}],"rules":["MATCH,other"]});
        let config = build(
            &source.to_string(),
            crossbow_core::Engine::Mihomo,
            "a",
            12345,
            "nonce",
        )
        .unwrap();
        assert_eq!(config.document["proxies"].as_array().unwrap().len(), 2);
        assert!(config.document.get("proxy-providers").is_none());
        assert!(config.document.get("listeners").is_none());
        assert_eq!(config.document["tun"]["enable"], false);
        assert_eq!(config.document["mixed-port"], 12345);
        assert_eq!(config.document["proxy-groups"][0]["proxies"][0], "a");
        assert_eq!(
            config.document["rules"][0],
            format!(
                "MATCH,{}",
                config.document["proxy-groups"][0]["name"].as_str().unwrap()
            )
        );
        assert_eq!(config.document["skip-auth-prefixes"], json!([]));
    }
    #[test]
    fn rejects_groups_remote_nodes_cycles_and_host_network_protocols() {
        for source in [
            json!({"proxies":[{"name":"x","type":"tailscale"}]}),
            json!({"proxies":[{"name":"x","type":"ss","dialer-proxy":"x"}]}),
            json!({"proxies":[],"proxy-groups":[{"name":"x","type":"select"}]}),
            json!({"proxies":[],"proxy-providers":{"x":{}}}),
        ] {
            assert!(build(
                &source.to_string(),
                crossbow_core::Engine::Mihomo,
                "x",
                1,
                "nonce"
            )
            .is_err());
        }
    }
    #[test]
    fn native_singbox_copies_only_dependency_chain_and_flags_http_udp_unsupported() {
        let source = json!({"inbounds":[{"type":"tun"}],"outbounds":[{"tag":"x","type":"http","server":"example.invalid","detour":"d"},{"tag":"d","type":"direct"}],"endpoints":[{"type":"tailscale"}]});
        let c = build(
            &source.to_string(),
            crossbow_core::Engine::SingBox,
            "x",
            1234,
            "nonce",
        )
        .unwrap();
        assert!(!c.udp_supported);
        assert!(c.document.get("endpoints").is_none());
        assert_eq!(c.document["inbounds"][0]["type"], "mixed");
        assert_eq!(c.document["route"]["final"], "x");
    }
}
