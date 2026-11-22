//! Persisted group intent for one profile's inline concrete proxies. No downloads.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Select,
    UrlTest,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Membership {
    Explicit,
    Filter,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub profile_id: String,
    pub name: String,
    pub kind: Kind,
    pub membership: Membership,
    pub members: Vec<String>,
    pub filter: String,
    pub url: String,
    pub interval: u32,
    pub tolerance: u16,
}
#[derive(Debug, Clone, Serialize)]
pub struct Resolution {
    pub members: Vec<String>,
    pub missing: Vec<String>,
    pub error: Option<String>,
}
pub fn validate(groups: &[Group]) -> Result<(), String> {
    if groups.len() > 32 {
        return Err("At most 32 custom groups / 自定义组最多 32 个".into());
    }
    let mut ids = BTreeSet::new();
    let mut names = BTreeSet::new();
    for g in groups {
        if g.id.is_empty()
            || g.id.len() > 128
            || !ids.insert(&g.id)
            || g.profile_id.is_empty()
            || g.name.trim() != g.name
            || g.name.is_empty()
            || g.name.chars().count() > 80
            || g.name.chars().any(|c| c == ',' || c.is_control())
            || matches!(
                g.name.as_str(),
                "DIRECT" | "REJECT" | "GLOBAL" | "COMPATIBLE"
            )
            || !names.insert((&g.profile_id, &g.name))
        {
            return Err("Invalid or duplicate group identity/name / 组名无效或重复".into());
        }
        if g.members.len() > 4096 || g.filter.len() > 256 {
            return Err("Group selection too large / 成员选择过大".into());
        }
        match g.membership {
            Membership::Explicit
                if g.members.is_empty()
                    || g.members.iter().collect::<BTreeSet<_>>().len() != g.members.len() =>
            {
                return Err("Select distinct concrete nodes / 请选择不重复的具体节点".into())
            }
            Membership::Filter if g.filter.trim().is_empty() => {
                return Err("Name filter is required / 请输入名称过滤条件".into())
            }
            _ => {}
        }
        if matches!(g.kind, Kind::UrlTest) {
            let u = url::Url::parse(&g.url).map_err(|_| "Invalid test URL / 测试地址无效")?;
            if !matches!(u.scheme(), "http" | "https")
                || u.host_str().is_none()
                || !u.username().is_empty()
                || u.password().is_some()
                || u.fragment().is_some()
                || g.url.len() > 2048
                || !(30..=86400).contains(&g.interval)
                || g.tolerance == 0
            {
                return Err("URL-test requires HTTP(S), interval 30..86400s, tolerance 1..65535ms / 自动测试参数超出双内核共同范围".into());
            }
        }
    }
    Ok(())
}
pub fn candidates(doc: &Value) -> Result<Vec<String>, String> {
    let list = doc["proxies"]
        .as_array()
        .ok_or("Only Clash-format inline proxies are supported / 首版仅支持 Clash 格式内联节点")?;
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for node in list {
        let name = node["name"]
            .as_str()
            .ok_or("Node has no name / 节点缺少名称")?;
        if !seen.insert(name) || name.is_empty() {
            return Err("Duplicate node names / 来源节点名称重复".into());
        }
        if !matches!(name, "DIRECT" | "REJECT" | "COMPATIBLE" | "GLOBAL")
            && !matches!(
                node["type"].as_str(),
                Some("direct" | "reject" | "dns" | "compatible")
            )
        {
            out.push(name.into());
        }
    }
    Ok(out)
}
pub fn resolve(doc: &Value, g: &Group) -> Resolution {
    let fail = |message: String, missing: Vec<String>| Resolution {
        members: vec![],
        missing,
        error: Some(message),
    };
    let available = match candidates(doc) {
        Ok(v) => v,
        Err(e) => return fail(e, vec![]),
    };
    if doc["proxies"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|node| node["name"] == g.name)
        || doc["proxy-groups"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|v| v["name"] == g.name)
    {
        return fail(
            "Name collides with a source node/group / 与来源节点或策略组重名".into(),
            vec![],
        );
    }
    let filter = g.filter.to_lowercase();
    let available_set = available.iter().collect::<BTreeSet<_>>();
    let members = match g.membership {
        Membership::Explicit => g.members.clone(),
        Membership::Filter => available
            .iter()
            .filter(|v| v.to_lowercase().contains(&filter))
            .cloned()
            .collect(),
    };
    let missing = members
        .iter()
        .filter(|m| !available_set.contains(m))
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return fail("Missing node references; nested groups and DIRECT are not supported / 节点引用失效；不支持嵌套组或 DIRECT 候选".into(),missing);
    }
    if members.is_empty() {
        return fail("No matching nodes / 解析后没有候选节点".into(), vec![]);
    }
    Resolution {
        members,
        missing,
        error: None,
    }
}
pub fn inject(config: &str, profile_id: &str, groups: &[Group]) -> Result<String, String> {
    let selected = groups
        .iter()
        .filter(|g| g.profile_id == profile_id)
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Ok(config.into());
    }
    validate(groups)?;
    let mut doc: Value =
        serde_yaml::from_str(config).map_err(|_| "Invalid source configuration / 来源配置无效")?;
    let mut added = Vec::new();
    for group in selected {
        let resolved = resolve(&doc, group);
        if let Some(error) = resolved.error {
            return Err(error);
        }
        let mut entry = json!({"name":group.name,"type":if matches!(group.kind,Kind::Select){"select"}else{"url-test"},"proxies":resolved.members});
        if matches!(group.kind, Kind::UrlTest) {
            entry["url"] = json!(group.url);
            entry["interval"] = json!(group.interval);
            entry["tolerance"] = json!(group.tolerance);
        }
        added.push(entry);
    }
    if doc.get("proxy-groups").is_none() {
        doc["proxy-groups"] = json!([]);
    }
    doc["proxy-groups"]
        .as_array_mut()
        .ok_or("Invalid source groups / 来源策略组格式无效")?
        .extend(added);
    serde_yaml::to_string(&doc).map_err(|_| "Group serialization failed / 策略组编码失败".into())
}
/// Flag enrichment may rename a node into a custom group's name. Check after
/// enrichment too, so a stopped core cannot persist a configuration it cannot start.
pub fn validate_effective_names(
    config: &str,
    profile_id: &str,
    groups: &[Group],
) -> Result<(), String> {
    let selected: BTreeSet<_> = groups
        .iter()
        .filter(|g| g.profile_id == profile_id)
        .map(|g| &g.name)
        .collect();
    if selected.is_empty() {
        return Ok(());
    }
    let doc: Value =
        serde_yaml::from_str(config).map_err(|_| "Invalid rendered source".to_string())?;
    let mut seen = BTreeSet::new();
    for item in doc["proxies"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(doc["proxy-groups"].as_array().into_iter().flatten())
    {
        if let Some(name) = item["name"].as_str() {
            if !seen.insert(name) && selected.iter().any(|g| g.as_str() == name) {
                return Err(
                    "Group name collides after flag enrichment / 旗帜补全后组名与节点重名".into(),
                );
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn group() -> Group {
        Group {
            id: "id".into(),
            profile_id: "p".into(),
            name: "Personal".into(),
            kind: Kind::Select,
            membership: Membership::Explicit,
            members: vec!["A".into()],
            filter: "".into(),
            url: "https://www.gstatic.com/generate_204".into(),
            interval: 300,
            tolerance: 50,
        }
    }
    const SOURCE:&str="proxies: [{name: A, type: http, server: 127.0.0.1, port: 1}, {name: B, type: http, server: 127.0.0.1, port: 2}]\nproxy-groups: [{name: Existing, type: select, proxies: [A]}]\nrules: ['MATCH,Existing']";
    #[test]
    fn injects_without_mutating_original_and_converts_both_kinds() {
        for kind in [Kind::Select, Kind::UrlTest] {
            let mut g = group();
            g.kind = kind;
            let result = inject(SOURCE, "p", &[g]).unwrap();
            assert!(result.contains("Personal"));
            assert!(result.contains("MATCH,Existing"));
            let sb = crate::convert_to_singbox(&result, 7897, false, None).unwrap();
            assert!(sb.contains("Personal"));
        }
        assert_eq!(inject(SOURCE, "other", &[group()]).unwrap(), SOURCE);
    }
    #[test]
    fn flag_enrichment_cannot_create_a_duplicate_custom_group() {
        let source = SOURCE
            .replace("name: A", "name: 香港")
            .replace("[A]", "[香港]");
        let mut g = group();
        g.name = "🇭🇰|香港".into();
        g.members = vec!["香港".into()];
        let injected = inject(&source, "p", &[g.clone()]).unwrap();
        assert!(validate_effective_names(&injected, "p", &[g.clone()]).is_ok());
        let enriched = crate::emoji::enrich_flags(&injected).unwrap();
        assert!(validate_effective_names(&enriched, "p", &[g]).is_err());
    }
    #[test]
    fn invalid_references_names_and_empty_filters_fail() {
        let d: Value = serde_yaml::from_str(SOURCE).unwrap();
        for name in ["DIRECT", "Personal", "Existing", "missing"] {
            let mut g = group();
            g.members = vec![name.into()];
            assert!(resolve(&d, &g).error.is_some());
        }
        let mut non_concrete = d.clone();
        non_concrete["proxies"]
            .as_array_mut()
            .unwrap()
            .push(json!({"name":"Personal","type":"direct"}));
        assert!(resolve(&non_concrete, &group()).error.is_some());
        let mut g = group();
        g.name = "Existing".into();
        assert!(resolve(&d, &g).error.is_some());
        g = group();
        g.membership = Membership::Filter;
        g.filter = "none".into();
        assert!(resolve(&d, &g).error.is_some());
    }
    #[test]
    fn filters_track_source_updates_and_zero_tolerance_rejected() {
        let d: Value = serde_yaml::from_str(SOURCE).unwrap();
        let mut g = group();
        g.membership = Membership::Filter;
        g.filter = "a".into();
        assert_eq!(resolve(&d, &g).members, ["A"]);
        let mut updated = d.clone();
        updated["proxies"]
            .as_array_mut()
            .unwrap()
            .push(json!({"name":"a-new","type":"http"}));
        assert_eq!(resolve(&updated, &g).members, ["A", "a-new"]);
        updated["proxies"].as_array_mut().unwrap().remove(0);
        assert_eq!(resolve(&updated, &g).members, ["a-new"]);
        let mut explicit = group();
        explicit.members = vec!["A".into()];
        let unavailable = resolve(&updated, &explicit);
        assert_eq!(unavailable.missing, ["A"]);
        assert!(unavailable.members.is_empty());
        assert!(inject(&serde_yaml::to_string(&updated).unwrap(), "p", &[explicit]).is_err());
        g.kind = Kind::UrlTest;
        g.tolerance = 0;
        assert!(validate(&[g]).is_err());
        let g = group();
        assert!(validate(&[g.clone(), g]).is_err());
    }
}

#[cfg(test)]
mod native_fixture {
    use super::*;
    #[test]
    #[ignore = "explicit isolated fixture exporter for controlled native checks"]
    fn export() {
        let input = std::env::var("CB_GROUP_INPUT").unwrap();
        let output = std::env::var("CB_GROUP_OUTPUT").unwrap();
        let v: Value = serde_json::from_slice(&std::fs::read(input).unwrap()).unwrap();
        let mut data = crate::StoreSchema::default();
        data.engine.engine = serde_json::from_value(v["engine"].clone()).unwrap();
        data.engine.flag_emoji = false;
        data.engine.mixed_port = v["port"].as_u64().unwrap() as u16;
        data.profiles.push(
            serde_json::from_value(
                json!({"id":"p","name":"Synthetic source","kind":"local","content":v["config"]}),
            )
            .unwrap(),
        );
        data.active_profile = Some("p".into());
        data.custom_groups = serde_json::from_value(v["groups"].clone()).unwrap();
        data.user_rules.push(crate::user_rules::UserRule {
            id: "routing".into(),
            kind: crate::user_rules::Kind::IpCidr,
            value: "127.0.0.1/32".into(),
            target: "Personal".into(),
            enabled: true,
        });
        let dir = tempfile::tempdir().unwrap();
        let mut store = crate::Store::open(dir.path()).unwrap();
        *store.data_mut() = data;
        store.save().unwrap();
        drop(store);
        let store = crate::Store::open(dir.path()).unwrap();
        let rendered = crate::render_config(store.data()).unwrap();
        let mut doc: Value = serde_yaml::from_str(&rendered.config).unwrap();
        if store.data().engine.engine == crate::Engine::Mihomo {
            doc["mixed-port"] = v["port"].clone();
            doc["bind-address"] = json!("127.0.0.1");
            doc["external-controller"] =
                json!(format!("127.0.0.1:{}", v["control"].as_u64().unwrap()));
            doc["secret"] = json!("synthetic");
        } else {
            doc["experimental"]["cache_file"]["enabled"] = json!(false);
            doc["experimental"]["clash_api"] = json!({"external_controller":format!("127.0.0.1:{}",v["control"].as_u64().unwrap()),"secret":"synthetic"});
        }
        std::fs::write(output, serde_json::to_vec(&doc).unwrap()).unwrap();
    }
}
