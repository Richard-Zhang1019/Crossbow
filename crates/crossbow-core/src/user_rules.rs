//! Ordered application rules: process > user > managed rule sets > subscription.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::net::IpAddr;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Kind {
    Domain,
    DomainSuffix,
    IpCidr,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserRule {
    pub id: String,
    pub kind: Kind,
    pub value: String,
    pub target: String,
    pub enabled: bool,
}

pub fn normalize(kind: &Kind, value: &str) -> Result<String, String> {
    let value = value.trim();
    if matches!(kind, Kind::IpCidr) {
        let (address, prefix) = value.split_once('/').ok_or("CIDR requires a prefix")?;
        let address: IpAddr = address.parse().map_err(|_| "Invalid IP address")?;
        let prefix: u32 = prefix.parse().map_err(|_| "Invalid CIDR prefix")?;
        return match address {
            IpAddr::V4(ip) if prefix <= 32 => Ok(format!(
                "{}/{}",
                std::net::Ipv4Addr::from(
                    u32::from(ip) & u32::MAX.checked_shl(32 - prefix).unwrap_or(0)
                ),
                prefix
            )),
            IpAddr::V6(ip) if prefix <= 128 => Ok(format!(
                "{}/{}",
                std::net::Ipv6Addr::from(
                    u128::from(ip) & u128::MAX.checked_shl(128 - prefix).unwrap_or(0)
                ),
                prefix
            )),
            _ => Err("Invalid CIDR prefix".into()),
        };
    }
    if value.is_empty()
        || value.len() > 253
        || value
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '/' | ':' | ',' | '*' | '\\'))
    {
        return Err("Invalid domain".into());
    }
    let domain = match url::Host::parse(value.strip_suffix('.').unwrap_or(value))
        .map_err(|_| "Invalid domain")?
    {
        url::Host::Domain(d) => d.to_ascii_lowercase(),
        _ => return Err("Use IP-CIDR for IP addresses".into()),
    };
    if domain.len() > 253
        || domain.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return Err("Invalid domain label".into());
    }
    Ok(domain)
}
pub fn validate(rules: &mut [UserRule]) -> Result<(), String> {
    if rules.len() > 512 {
        return Err("At most 512 user rules".into());
    }
    let mut keys = std::collections::HashSet::new();
    let mut ids = std::collections::HashSet::new();
    for rule in rules {
        rule.value = normalize(&rule.kind, &rule.value)?;
        if rule.id.is_empty()
            || rule.id.len() > 128
            || !ids.insert(rule.id.clone())
            || rule.target.is_empty()
            || rule.target.len() > 256
            || rule.target.chars().any(|c| c == ',' || c.is_control())
        {
            return Err("Invalid rule identity or target".into());
        }
        if !keys.insert((format!("{:?}", rule.kind), rule.value.clone())) {
            return Err("Duplicate user rule".into());
        }
    }
    Ok(())
}
pub fn targets(doc: &Value) -> Vec<String> {
    let mut result = vec!["DIRECT".into(), "REJECT".into()];
    for (section, key) in [("proxy-groups", "name"), ("outbounds", "tag")] {
        for item in doc[section].as_array().into_iter().flatten() {
            if let Some(name) = item[key].as_str() {
                if !result.iter().any(|n| n == name) {
                    result.push(name.into());
                }
            }
        }
    }
    result
}
/// Invalid persisted entries and missing policies fail closed by not injecting.
pub fn inject(config: &str, rules: &[UserRule], is_json: bool) -> Result<String, String> {
    if rules.is_empty() {
        return Ok(config.into());
    }
    let mut doc: Value = serde_yaml::from_str(config).map_err(|_| "Invalid configuration")?;
    let names = targets(&doc);
    let mut added = Vec::new();
    for rule in rules.iter().filter(|r| r.enabled) {
        let Ok(value) = normalize(&rule.kind, &rule.value) else {
            continue;
        };
        if !names.contains(&rule.target) || rule.target.chars().any(|c| c == ',' || c.is_control())
        {
            continue;
        }
        if is_json {
            let key = match rule.kind {
                Kind::Domain => "domain",
                Kind::DomainSuffix => "domain_suffix",
                Kind::IpCidr => "ip_cidr",
            };
            let mut entry = if matches!(rule.kind, Kind::DomainSuffix) {
                json!({"domain":[value],"domain_suffix":[format!(".{value}")]})
            } else {
                json!({key:[value]})
            };
            // Empty/default Clash mode is rule mode; explicit direct/global
            // bypass application user rules even for native sing-box profiles.
            entry = json!({"type":"logical","mode":"and","rules":[entry,{"clash_mode":"direct","invert":true},{"clash_mode":"global","invert":true}]});
            if rule.target == "REJECT" {
                entry["action"] = json!("reject");
            } else {
                entry["outbound"] = json!(rule.target);
            }
            if rule.target == "DIRECT"
                && !doc["outbounds"]
                    .as_array()
                    .is_some_and(|v| v.iter().any(|v| v["tag"] == "DIRECT"))
            {
                doc["outbounds"]
                    .as_array_mut()
                    .ok_or("Missing outbounds")?
                    .push(json!({"type":"direct","tag":"DIRECT"}));
            }
            added.push(entry);
        } else {
            let kind = match rule.kind {
                Kind::Domain => "DOMAIN",
                Kind::DomainSuffix => "DOMAIN-SUFFIX",
                Kind::IpCidr if value.contains(':') => "IP-CIDR6",
                Kind::IpCidr => "IP-CIDR",
            };
            added.push(json!(format!(
                "{kind},{value},{}{}",
                rule.target,
                if matches!(rule.kind, Kind::IpCidr) {
                    ",no-resolve"
                } else {
                    ""
                }
            )));
        }
    }
    let parent = if is_json {
        if doc.get("route").is_none() {
            doc["route"] = json!({});
        }
        doc.get_mut("route").ok_or("Invalid route")?
    } else {
        &mut doc
    };
    if !parent.is_object() {
        return Err("Invalid rules container".into());
    }
    let old = parent["rules"].as_array().cloned().unwrap_or_default();
    added.extend(old);
    parent["rules"] = json!(added);
    if is_json {
        serde_json::to_string(&doc).map_err(|_| "Serialize failed".into())
    } else {
        serde_yaml::to_string(&doc).map_err(|_| "Serialize failed".into())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boundaries() {
        assert_eq!(
            normalize(&Kind::Domain, "EXAMPLE.com.").unwrap(),
            "example.com"
        );
        assert_eq!(
            normalize(&Kind::IpCidr, "2001:db8::1234/64").unwrap(),
            "2001:db8::/64"
        );
        assert_eq!(normalize(&Kind::IpCidr, "10.1.2.3/0").unwrap(), "0.0.0.0/0");
        for v in [
            "a,b",
            "*.example.com",
            "https://example.com",
            "127.0.0.1",
            "a..b",
            "-a.com",
        ] {
            assert!(normalize(&Kind::Domain, v).is_err());
        }
        assert!(normalize(&Kind::IpCidr, "::1/129").is_err());
    }
    #[test]
    fn injection_and_stale_disabled_targets() {
        let mut rules = vec![UserRule {
            id: "one".into(),
            kind: Kind::DomainSuffix,
            value: "Example.com".into(),
            target: "DIRECT".into(),
            enabled: true,
        }];
        validate(&mut rules).unwrap();
        let yaml = inject("proxies: []\nrules: ['MATCH,REJECT']", &rules, false).unwrap();
        assert!(yaml.find("DOMAIN-SUFFIX").unwrap() < yaml.find("MATCH").unwrap());
        let json = inject(r#"{"outbounds":[],"route":{"rules":[]}}"#, &rules, true).unwrap();
        let doc: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            doc["route"]["rules"][0]["rules"][0]["domain_suffix"][0],
            ".example.com"
        );
        assert_eq!(doc["outbounds"][0]["tag"], "DIRECT");
        rules.push(rules[0].clone());
        assert!(validate(&mut rules).is_err());
        rules.pop();
        rules[0].target = "gone".into();
        assert!(!inject(&yaml, &rules, false)
            .unwrap()
            .contains("DOMAIN-SUFFIX,example.com,gone"));
    }
}

#[cfg(test)]
mod native_fixture {
    #[test]
    #[ignore = "explicit isolated fixture paths for native verification"]
    fn export() {
        let input = std::env::var("CB_USER_RULE_INPUT").unwrap();
        let output = std::env::var("CB_USER_RULE_OUTPUT").unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(input).unwrap()).unwrap();
        let mut store = crate::StoreSchema::default();
        store.engine.engine = serde_json::from_value(value["engine"].clone()).unwrap();
        store.engine.mixed_port = value["port"].as_u64().unwrap() as u16;
        store.user_rules = serde_json::from_value(value["rules"].clone()).unwrap();
        super::validate(&mut store.user_rules).unwrap();
        store.profiles.push(crate::Profile {
            id: "test".into(),
            name: "synthetic".into(),
            kind: crate::ProfileKind::Local,
            url: None,
            content: value["config"].as_str().unwrap().into(),
            update_interval_min: 0,
            override_ids: vec![],
            last_updated: None,
            traffic: None,
            last_update: None,
        });
        store.active_profile = Some("test".into());
        let rendered = crate::render_config(&store).unwrap();
        let mut doc: serde_json::Value = serde_yaml::from_str(&rendered.config).unwrap();
        if store.engine.engine == crate::Engine::Mihomo {
            doc["mixed-port"] = value["port"].clone();
            doc["bind-address"] = serde_json::json!("127.0.0.1");
            doc["external-controller"] = serde_json::json!("");
        } else {
            doc["experimental"]["cache_file"]["enabled"] = serde_json::json!(false);
        }
        std::fs::write(output, serde_json::to_vec(&doc).unwrap()).unwrap();
    }
}

/// Limited static coverage, not a simulation of core routing or DNS.
pub fn overlaps(rules: &[UserRule], available: &[String]) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    for (i, rule) in rules
        .iter()
        .enumerate()
        .filter(|(_, r)| r.enabled && available.contains(&r.target))
    {
        for (j, earlier) in rules[..i]
            .iter()
            .enumerate()
            .filter(|(_, r)| r.enabled && available.contains(&r.target))
        {
            let domain = matches!(earlier.kind, Kind::DomainSuffix)
                && matches!(rule.kind, Kind::Domain | Kind::DomainSuffix)
                && (rule.value == earlier.value
                    || rule.value.ends_with(&format!(".{}", earlier.value)));
            if domain {
                pairs.push((i, j));
                break;
            }
        }
    }
    pairs
}
#[cfg(test)]
mod overlap_tests {
    use super::*;
    #[test]
    fn suffix_coverage_has_label_boundary_and_ignores_disabled_rules() {
        let mut rules = vec![
            UserRule {
                id: "a".into(),
                kind: Kind::DomainSuffix,
                value: "example.com".into(),
                target: "DIRECT".into(),
                enabled: true,
            },
            UserRule {
                id: "b".into(),
                kind: Kind::Domain,
                value: "sub.example.com".into(),
                target: "REJECT".into(),
                enabled: true,
            },
        ];
        assert_eq!(
            overlaps(&rules, &["DIRECT".into(), "REJECT".into()]),
            vec![(1, 0)]
        );
        rules[1].value = "notexample.com".into();
        assert!(overlaps(&rules, &["DIRECT".into(), "REJECT".into()]).is_empty());
    }
}
