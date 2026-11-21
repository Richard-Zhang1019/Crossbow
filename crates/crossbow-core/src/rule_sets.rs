//! Application-managed rule subscriptions. Cache and metadata live atomically in
//! the local store; neither engine downloads these providers during startup.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;

pub const MAX_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_SETS: usize = 32;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Behavior {
    Domain,
    Ipcidr,
    Classical,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleSet {
    pub id: String,
    pub name: String,
    pub url: String,
    pub behavior: Behavior,
    pub target: String,
    pub enabled: bool,
    pub interval_secs: u32,
    pub payload: Vec<String>,
    pub updated_at: u64,
    #[serde(default)]
    pub last_error: Option<String>,
}
pub fn canonical_url(raw: &str) -> Result<String, String> {
    let mut url = url::Url::parse(raw.trim()).map_err(|_| "规则集 URL 无效")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("规则集仅支持不含用户凭据的 HTTP(S) URL".into());
    }
    url.set_fragment(None);
    Ok(url.to_string())
}
fn domain(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
        })
}

fn cidr(value: &str) -> bool {
    let Some((address, prefix)) = value.split_once('/') else {
        return false;
    };
    match (address.parse::<std::net::IpAddr>(), prefix.parse::<u8>()) {
        (Ok(std::net::IpAddr::V4(_)), Ok(p)) => p <= 32,
        (Ok(std::net::IpAddr::V6(_)), Ok(p)) => p <= 128,
        _ => false,
    }
}
/// Strict common subset. Never skip an unsupported entry to make a set load.
fn headless(behavior: Behavior, entry: &str) -> Result<Value, String> {
    if behavior == Behavior::Domain {
        let normalized = entry.to_ascii_lowercase();
        let entry = normalized.as_str();
        let suffix = entry.strip_prefix("+.");
        if let Some(suffix) = suffix.filter(|v| domain(v)) {
            return Ok(json!({"domain_suffix":[suffix]}));
        }
        if let Some(suffix) = entry.strip_prefix('.').filter(|v| domain(v)) {
            // A leading dot excludes the apex in mihomo. domain_suffix alone
            // would incorrectly include it in sing-box.
            return Ok(json!({"domain_regex":[format!("^.+\\.{}$",suffix.replace('.',"\\."))]}));
        }
        if entry.len() <= 253
            && entry
                .split('.')
                .all(|v| v == "*" || domain(v) && !v.contains('.'))
            && entry.contains('*')
        {
            let pattern = entry
                .split('.')
                .map(|v| if v == "*" { "[^.]+" } else { v })
                .collect::<Vec<_>>()
                .join("\\.");
            return Ok(json!({"domain_regex":[format!("^{pattern}$")]}));
        }
        if domain(entry) {
            return Ok(json!({"domain":[entry]}));
        }
        return Err("域名或通配符无效".into());
    }
    let (kind, value) = match behavior {
        Behavior::Ipcidr => ("IP-CIDR", entry),
        Behavior::Classical => entry.split_once(',').ok_or("classical 缺少规则类型")?,
        Behavior::Domain => return Err("域名无效".into()),
    };
    let kind = kind.trim();
    let value = value.trim();
    if value.is_empty() || value.contains(',') || value.chars().any(char::is_control) {
        return Err("规则参数为空或包含不支持的附加参数".into());
    }
    Ok(match kind {
        "DOMAIN" | "DOMAIN-SUFFIX" if domain(value) => {
            let key = if kind == "DOMAIN" {
                "domain"
            } else {
                "domain_suffix"
            };
            json!({key: [value]})
        }
        "DOMAIN-KEYWORD" if !value.is_empty() => json!({"domain_keyword":[value]}),
        "IP-CIDR" | "IP-CIDR6" if cidr(value) => json!({"ip_cidr":[value]}),
        "SRC-IP-CIDR" if cidr(value) => json!({"source_ip_cidr":[value]}),
        "DST-PORT" | "SRC-PORT" => {
            let port = value
                .parse::<u16>()
                .ok()
                .filter(|p| *p > 0)
                .ok_or("端口无效")?;
            let key = if kind == "DST-PORT" {
                "port"
            } else {
                "source_port"
            };
            json!({key:[port]})
        }
        "NETWORK" if matches!(value, "tcp" | "udp") => json!({"network":[value]}),
        "PROCESS-NAME" if crate::process_rules::valid_process_name(value) => {
            json!({"process_name":[value]})
        }
        _ => return Err("规则类型、通配符或匹配参数不能在两个内核间等价转换".into()),
    })
}
pub fn parse_payload(body: &str, behavior: Behavior) -> Result<Vec<String>, String> {
    if body.len() > MAX_BYTES {
        return Err("规则集超过 2 MiB".into());
    }
    let text = body.trim().trim_start_matches('\u{feff}').trim();
    let entries = match serde_yaml::from_str::<Value>(text) {
        Ok(Value::Object(doc)) => doc
            .get("payload")
            .and_then(Value::as_array)
            .ok_or("规则集 YAML 必须包含 payload 列表")?
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or("payload 条目必须是字符串".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?,
        Ok(Value::Array(_)) => return Err("YAML 规则集需要 payload 包装".into()),
        _ => text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("//"))
            .map(str::to_owned)
            .collect(),
    };
    if entries.is_empty() || entries.len() > 50_000 {
        return Err("规则集需包含 1–50000 条规则".into());
    }
    let mut seen = HashSet::new();
    let mut payload = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let entry = entry.trim();
        headless(behavior, entry).map_err(|e| format!("规则集第 {} 条：{e}", index + 1))?;
        if seen.insert(entry.to_owned()) {
            payload.push(entry.to_owned());
        }
    }
    Ok(payload)
}
pub fn fetch(url: &str, behavior: Behavior) -> Result<Vec<String>, String> {
    use std::io::Read;
    let url = canonical_url(url)?;
    let response = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .get(&url)
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(code, _) => format!("规则集下载失败：HTTP {code}"),
            ureq::Error::Transport(e) => format!("规则集下载失败：{:?}", e.kind()),
        })?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "规则集读取失败")?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| "规则集不是 UTF-8 文本（不支持 MRS/SRS 二进制）")?;
    parse_payload(text, behavior)
}
pub fn targets(doc: &Value) -> Vec<String> {
    let mut names = vec!["DIRECT".into(), "REJECT".into()];
    for (section, key) in [
        ("proxy-groups", "name"),
        ("proxies", "name"),
        ("outbounds", "tag"),
    ] {
        for item in doc[section].as_array().into_iter().flatten() {
            if let Some(name) = item[key].as_str() {
                if !names.iter().any(|n| n == name) {
                    names.push(name.to_owned());
                }
            }
        }
    }
    names
}
fn usable<'a>(doc: &Value, sets: &'a [RuleSet]) -> Result<Vec<&'a RuleSet>, String> {
    let targets = targets(doc);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for set in sets
        .iter()
        .filter(|s| s.enabled && targets.contains(&s.target))
    {
        if set.id.is_empty()
            || set.id.len() > 80
            || !set
                .id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
            || !seen.insert(&set.id)
        {
            return Err("规则集标识无效或重复".into());
        }
        if set.payload.is_empty() {
            return Err("启用的规则集没有缓存".into());
        }
        for entry in &set.payload {
            headless(set.behavior, entry)?;
        }
        out.push(set);
    }
    Ok(out)
}
fn tag(set: &RuleSet) -> String {
    format!("crossbow-ruleset-{}", set.id)
}
pub fn inject_yaml(config: &str, sets: &[RuleSet]) -> Result<String, String> {
    if sets.is_empty() {
        return Ok(config.into());
    }
    let mut doc: Value = serde_yaml::from_str(config).map_err(|_| "配置 YAML 无效")?;
    if !doc.is_object() {
        return Err("配置必须为对象".into());
    }
    let enabled = usable(&doc, sets)?;
    if enabled.is_empty() {
        return Ok(config.into());
    }
    if doc.get("rule-providers").is_some_and(|v| !v.is_object()) {
        return Err("rule-providers 必须为对象".into());
    }
    let mut rules = Vec::new();
    for set in enabled {
        let name = tag(set);
        if doc["rule-providers"].get(&name).is_some() {
            return Err("规则集名称与订阅 provider 冲突".into());
        }
        doc["rule-providers"][&name] =
            json!({"type":"inline","behavior":set.behavior,"payload":set.payload});
        rules.push(json!(format!("RULE-SET,{name},{}", set.target)));
    }
    if let Some(existing) = doc.get("rules") {
        rules.extend(existing.as_array().ok_or("rules 必须为列表")?.clone());
    }
    doc["rules"] = json!(rules);
    serde_yaml::to_string(&doc).map_err(|_| "规则集配置编码失败".into())
}
pub fn inject_json(config: &str, sets: &[RuleSet]) -> Result<String, String> {
    if sets.is_empty() {
        return Ok(config.into());
    }
    let mut doc: Value = serde_json::from_str(config).map_err(|_| "配置 JSON 无效")?;
    if !doc.is_object() {
        return Err("配置必须为对象".into());
    }
    let enabled = usable(&doc, sets)?;
    if enabled.is_empty() {
        return Ok(config.into());
    }
    if doc.get("route").is_some_and(|v| !v.is_object()) {
        return Err("route 必须为对象".into());
    }
    if doc["route"].get("rule_set").is_some_and(|v| !v.is_array()) {
        return Err("route.rule_set 必须为列表".into());
    }
    let mut defs = doc["route"]["rule_set"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut rules = Vec::new();
    for set in enabled {
        let name = tag(set);
        if defs.iter().any(|v| v["tag"] == name) {
            return Err("规则集名称与订阅 rule_set 冲突".into());
        }
        let payload = set
            .payload
            .iter()
            .map(|v| headless(set.behavior, v))
            .collect::<Result<Vec<_>, _>>()?;
        defs.push(json!({"type":"inline","tag":name,"rules":payload}));
        rules.push(json!({"rule_set":[name],"outbound":set.target}));
        if set.target == "REJECT"
            && !doc["outbounds"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|v| v["tag"] == "REJECT")
        {
            doc["outbounds"]
                .as_array_mut()
                .ok_or("outbounds 必须为列表")?
                .push(json!({"type":"block","tag":"REJECT"}));
        }
    }
    if let Some(existing) = doc["route"].get("rules") {
        rules.extend(existing.as_array().ok_or("route.rules 必须为列表")?.clone());
    }
    doc["route"]["rule_set"] = json!(defs);
    doc["route"]["rules"] = json!(rules);
    serde_json::to_string(&doc).map_err(|_| "规则集配置编码失败".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn set(id: &str, behavior: Behavior, entry: &str, target: &str) -> RuleSet {
        RuleSet {
            id: id.into(),
            name: id.into(),
            url: format!("https://example.invalid/{id}"),
            behavior,
            target: target.into(),
            enabled: true,
            interval_secs: 3600,
            payload: parse_payload(entry, behavior).unwrap(),
            updated_at: 100,
            last_error: None,
        }
    }
    const BASE:&str="proxies: []\nproxy-groups: [{name: Policy, type: select, proxies: [DIRECT]}]\nrules: [\"MATCH,DIRECT\"]\n";
    fn store() -> crate::StoreSchema {
        let mut store = crate::StoreSchema::default();
        store.profiles.push(crate::Profile {
            id: "p".into(),
            name: "Test".into(),
            kind: crate::ProfileKind::Local,
            url: None,
            content: BASE.into(),
            update_interval_min: 0,
            override_ids: vec![],
            last_updated: None,
            traffic: None,
            last_update: None,
        });
        store.active_profile = Some("p".into());
        store.process_rules.push(crate::ProcessRule {
            id: "process".into(),
            process: "Bank".into(),
            target: "DIRECT".into(),
        });
        store.rule_sets = vec![
            set(
                "ai",
                Behavior::Domain,
                "payload: ['+.ai.example', 'api.example']",
                "Policy",
            ),
            set("local", Behavior::Ipcidr, "10.0.0.0/8\n::1/128", "DIRECT"),
        ];
        store
    }
    #[test]
    fn strict_formats_and_no_silent_entry_loss() {
        assert_eq!(
            parse_payload(
                "# comment\n+.example.com\n+.example.com\n",
                Behavior::Domain
            )
            .unwrap(),
            vec!["+.example.com"]
        );
        for (behavior, text) in [
            (
                Behavior::Domain,
                "payload: [example.com, 'bad*partial.example.com']",
            ),
            (Behavior::Ipcidr, "192.0.2.1/33"),
            (
                Behavior::Classical,
                "DOMAIN,valid.example\nIP-CIDR,10.0.0.0/8,no-resolve",
            ),
            (Behavior::Classical, "MATCH,DIRECT"),
            (Behavior::Domain, "<html>error</html>"),
            (Behavior::Domain, "payload: [42]"),
            (Behavior::Domain, "payload: []"),
        ] {
            assert!(parse_payload(text, behavior).is_err());
        }
        for input in [
            "DOMAIN,example.com",
            "DOMAIN-SUFFIX,example.com",
            "DOMAIN-KEYWORD,service",
            "IP-CIDR6,::1/128",
            "SRC-IP-CIDR,10.0.0.0/8",
            "DST-PORT,443",
            "SRC-PORT,5000",
            "NETWORK,udp",
            "PROCESS-NAME,curl",
        ] {
            assert!(parse_payload(input, Behavior::Classical).is_ok());
        }
        assert_eq!(
            canonical_url(" https://EXAMPLE.com:443/list#fragment ").unwrap(),
            "https://example.com/list"
        );
        assert!(canonical_url("file:///tmp/list").is_err());
        assert!(canonical_url("https://secret:pass@example.com/list").is_err());
        assert_eq!(
            headless(Behavior::Domain, "*.EXAMPLE.COM").unwrap(),
            headless(Behavior::Domain, "*.example.com").unwrap()
        );
        assert_eq!(
            headless(Behavior::Domain, ".example.com").unwrap(),
            json!({"domain_regex":["^.+\\.example\\.com$"]})
        );
        assert_eq!(
            headless(Behavior::Domain, "*.*.example.com").unwrap(),
            json!({"domain_regex":["^[^.]+\\.[^.]+\\.example\\.com$"]})
        );
        assert_eq!(
            headless(Behavior::Domain, "+.example.com").unwrap(),
            json!({"domain_suffix":["example.com"]})
        );
    }
    #[test]
    fn render_priority_cache_order_switch_and_lifecycle() {
        let mut data = store();
        let mihomo = crate::render_config(&data).unwrap();
        let doc: Value = serde_yaml::from_str(&mihomo.config).unwrap();
        assert_eq!(doc["rules"][0], "PROCESS-NAME,Bank,DIRECT");
        assert_eq!(doc["rules"][1], "RULE-SET,crossbow-ruleset-ai,Policy");
        assert_eq!(doc["rules"][3], "MATCH,DIRECT");
        assert_eq!(
            doc["rule-providers"]["crossbow-ruleset-ai"]["type"],
            "inline"
        );
        data.engine.engine = crate::Engine::SingBox;
        let doc: Value =
            serde_json::from_str(&crate::render_config(&data).unwrap().config).unwrap();
        assert_eq!(doc["route"]["rules"][0]["process_name"][0], "Bank");
        assert_eq!(
            doc["route"]["rules"][1]["rule_set"][0],
            "crossbow-ruleset-ai"
        );
        assert_eq!(
            doc["route"]["rule_set"][0]["rules"][0]["domain_suffix"][0],
            "ai.example"
        );
        assert_eq!(
            doc["route"]["rule_set"][1]["rules"][0]["ip_cidr"][0],
            "10.0.0.0/8"
        );
        data.rule_sets.swap(0, 1);
        let doc: Value =
            serde_json::from_str(&crate::render_config(&data).unwrap().config).unwrap();
        assert_eq!(
            doc["route"]["rules"][1]["rule_set"][0],
            "crossbow-ruleset-local"
        );
        data.rule_sets[0].enabled = false;
        data.rule_sets[1].target = "Missing".into();
        let doc: Value =
            serde_json::from_str(&crate::render_config(&data).unwrap().config).unwrap();
        assert!(doc["route"].get("rule_set").is_none());
        let reopened: crate::StoreSchema =
            serde_json::from_str(&serde_json::to_string(&data).unwrap()).unwrap();
        assert_eq!(reopened.rule_sets, data.rule_sets);
        data.rule_sets.clear();
        assert!(crate::render_config(&data).is_ok());
        let old: crate::StoreSchema =
            serde_json::from_str(r#"{"profiles":[],"overrides":[]}"#).unwrap();
        assert!(old.rule_sets.is_empty());
    }
    #[test]
    fn native_singbox_profiles_keep_subscription_rules_and_attach_cached_sets() {
        let mut data = store();
        data.engine.engine = crate::Engine::SingBox;
        data.profiles[0].content=r#"{"outbounds":[{"type":"selector","tag":"Policy","outbounds":["DIRECT"]},{"type":"direct","tag":"DIRECT"}],"route":{"rules":[{"domain":["subscription.example"],"outbound":"DIRECT"}],"final":"DIRECT"}}"#.into();
        let doc: Value =
            serde_json::from_str(&crate::render_config(&data).unwrap().config).unwrap();
        assert_eq!(doc["outbounds"].as_array().unwrap().len(), 2);
        assert_eq!(doc["route"]["rules"][0]["process_name"][0], "Bank");
        assert_eq!(
            doc["route"]["rules"][1]["rule_set"][0],
            "crossbow-ruleset-ai"
        );
        assert_eq!(
            doc["route"]["rules"][3]["domain"][0],
            "subscription.example"
        );
    }
    #[test]
    fn providers_do_not_overwrite_and_reject_target_is_declared() {
        let active = set("valid", Behavior::Domain, "example.com", "DIRECT");
        assert!(inject_yaml("[]", std::slice::from_ref(&active)).is_err());
        assert!(inject_json("[]", std::slice::from_ref(&active)).is_err());
        for invalid in [".", "-example.com", "example..com"] {
            assert!(parse_payload(invalid, Behavior::Domain).is_err());
        }
        let reject = set(
            "reject",
            Behavior::Classical,
            "DOMAIN-SUFFIX,example.com",
            "REJECT",
        );
        let input = r#"{"outbounds":[{"tag":"DIRECT","type":"direct"}],"route":{}}"#;
        let doc: Value =
            serde_json::from_str(&inject_json(input, std::slice::from_ref(&reject)).unwrap())
                .unwrap();
        assert!(doc["outbounds"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["tag"] == "REJECT"));
        let yaml=format!("{BASE}rule-providers: {{crossbow-ruleset-reject: {{type: inline, behavior: domain, payload: [other.example]}}}}\n");
        assert!(inject_yaml(&yaml, &[reject]).is_err());
    }
    #[test]
    fn real_http_updates_validate_before_cache_replacement_and_redact_errors() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for body in [
                "payload: ['+.first.example']",
                "payload: ['+.second.example']",
                "payload: ['+.valid.example','bad*partial.unsupported.example']",
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut bytes = [0; 4096];
                assert!(stream.read(&mut bytes).unwrap() > 0);
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            }
        });
        let url = format!("http://{address}/PRIVATE_PATH?token=PRIVATE_TOKEN");
        let mut cached = fetch(&url, Behavior::Domain).unwrap();
        let previous = cached.clone();
        cached = fetch(&url, Behavior::Domain).unwrap();
        assert_ne!(cached, previous);
        let previous = cached.clone();
        let error = fetch(&url, Behavior::Domain).unwrap_err();
        assert!(!error.contains("PRIVATE_"));
        assert!(!error.contains("unsupported.example"));
        assert_eq!(cached, previous);
        server.join().unwrap();
    }
    #[test]
    #[ignore = "requires CROSSBOW_RULE_MIHOMO and CROSSBOW_RULE_SINGBOX"]
    fn native_engines_check_cached_inline_rule_sets() {
        let dir = tempfile::tempdir().unwrap();
        let mut data = store();
        data.rule_sets.push(set("classical",Behavior::Classical,"DOMAIN-SUFFIX,blocked.example\nSRC-IP-CIDR,192.0.2.0/24\nDST-PORT,1234\nSRC-PORT,5678\nNETWORK,udp\nPROCESS-NAME,test-process","REJECT"));
        for (engine, variable) in [
            (crate::Engine::Mihomo, "CROSSBOW_RULE_MIHOMO"),
            (crate::Engine::SingBox, "CROSSBOW_RULE_SINGBOX"),
        ] {
            data.engine.engine = engine;
            let config = crate::render_config(&data).unwrap().config;
            let path = dir.path().join(if engine == crate::Engine::Mihomo {
                "config.yaml"
            } else {
                "config.json"
            });
            if let Ok(output) = std::env::var("CROSSBOW_RULE_FIXTURES") {
                let root = std::path::Path::new(&output);
                std::fs::create_dir_all(root).unwrap();
                let mut disabled = data.clone();
                for set in &mut disabled.rule_sets {
                    set.enabled = false;
                }
                let disabled = crate::render_config(&disabled).unwrap().config;
                let disabled: Value = serde_yaml::from_str(&disabled).unwrap();
                std::fs::write(
                    root.join(if engine == crate::Engine::Mihomo {
                        "mihomo-disabled.json"
                    } else {
                        "singbox-disabled.json"
                    }),
                    serde_json::to_string_pretty(&disabled).unwrap(),
                )
                .unwrap();
                let doc: Value = serde_yaml::from_str(&config).unwrap();
                std::fs::write(
                    root.join(if engine == crate::Engine::Mihomo {
                        "mihomo.json"
                    } else {
                        "singbox.json"
                    }),
                    serde_json::to_string_pretty(&doc).unwrap(),
                )
                .unwrap();
            }
            std::fs::write(&path, config).unwrap();
            let mut command =
                std::process::Command::new(std::env::var(variable).expect("set binary path"));
            if engine == crate::Engine::Mihomo {
                command.arg("-t");
            } else {
                command.arg("check");
            }
            let out = command
                .arg(if engine == crate::Engine::Mihomo {
                    "-d"
                } else {
                    "-D"
                })
                .arg(dir.path())
                .arg(if engine == crate::Engine::Mihomo {
                    "-f"
                } else {
                    "-c"
                })
                .arg(path)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{engine:?}: {} {}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
}
