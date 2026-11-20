//! 按进程分流规则注入（#570）：`PROCESS-NAME` 规则前插到订阅规则之前。
//!
//! 渲染时注入而非生成覆写：merge 覆写的列表语义是替换、安全模式会跳过
//! 覆写链——进程规则是应用自身的结构化规则，不依赖覆写可用性。
//! 目标策略在渲染时校验：DIRECT/REJECT 恒有效；策略组名来自当前配置，
//! 订阅更新后组名消失的规则**防御性丢弃**（保留会让内核启动失败）。

use serde_json::Value as Json;
use serde_yaml::Value as Yaml;

use crate::model::ProcessRule;

/// 当前配置可用的策略目标名（mihomo 路径：proxy-groups 名；调用方恒补
/// DIRECT/REJECT）。
fn group_names(doc: &Yaml) -> Vec<String> {
    doc.get("proxy-groups")
        .and_then(|g| g.as_sequence())
        .map(|groups| {
            groups
                .iter()
                .filter_map(|g| g.get("name").and_then(|n| n.as_str()))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// CSV rule fields cannot contain separators or control characters.
pub fn valid_process_name(name: &str) -> bool {
    !name.trim().is_empty()
        && name.chars().count() <= 128
        && !name.chars().any(|c| c == ',' || c.is_control())
}

/// 校验并筛出可注入的规则：未知策略组的规则丢弃（返回被丢弃数供日志）。
fn usable(
    rules: &[ProcessRule],
    valid_targets: &dyn Fn(&str) -> bool,
) -> (Vec<(String, String)>, usize) {
    let mut ok = Vec::new();
    let mut dropped = 0;
    for rule in rules {
        if valid_process_name(&rule.process)
            && !rule.target.chars().any(|c| c == ',' || c.is_control())
            && valid_targets(&rule.target)
        {
            ok.push((rule.process.clone(), rule.target.clone()));
        } else {
            dropped += 1;
        }
    }
    (ok, dropped)
}

/// mihomo 路径：YAML 的 `rules` 列表前插 `PROCESS-NAME,<进程>,<目标>`。
/// 无 `rules` 键则创建。
pub fn inject_yaml(config_yaml: &str, rules: &[ProcessRule]) -> Result<String, serde_yaml::Error> {
    if rules.is_empty() {
        return Ok(config_yaml.to_string());
    }
    let mut doc: Yaml = serde_yaml::from_str(config_yaml)?;
    let names = group_names(&doc);
    let valid = |target: &str| {
        target == "DIRECT" || target == "REJECT" || names.contains(&target.to_string())
    };
    let (usable_rules, dropped) = usable(rules, &valid);
    if usable_rules.is_empty() {
        if dropped > 0 {
            eprintln!("process rules: {dropped} rule(s) dropped, target group no longer exists");
        }
        return Ok(config_yaml.to_string());
    }
    if dropped > 0 {
        eprintln!("process rules: {dropped} rule(s) dropped, target group no longer exists");
    }
    if !matches!(doc, Yaml::Mapping(_)) {
        doc = Yaml::Mapping(serde_yaml::Mapping::new());
    }
    let existing = doc
        .get("rules")
        .and_then(|r| r.as_sequence())
        .cloned()
        .unwrap_or_default();
    let mut merged: Vec<Yaml> = usable_rules
        .iter()
        .map(|(process, target)| Yaml::String(format!("PROCESS-NAME,{process},{target}")))
        .collect();
    merged.extend(existing);
    doc["rules"] = Yaml::Sequence(merged);
    serde_yaml::to_string(&doc)
}

/// sing-box 路径：`route.rules` 数组头部插入 `{"process_name": [...], "outbound": target}`。
pub fn inject_json(config_json: &str, rules: &[ProcessRule]) -> Result<String, serde_json::Error> {
    if rules.is_empty() {
        return Ok(config_json.to_string());
    }
    let mut doc: Json = serde_json::from_str(config_json)?;
    let names: Vec<String> = doc["outbounds"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|o| o.get("tag").and_then(|t| t.as_str()))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    let valid = |target: &str| {
        target == "DIRECT" || target == "REJECT" || names.contains(&target.to_string())
    };
    let (usable_rules, dropped) = usable(rules, &valid);
    if usable_rules.is_empty() {
        if dropped > 0 {
            eprintln!("process rules: {dropped} rule(s) dropped, target outbound no longer exists");
        }
        return Ok(config_json.to_string());
    }
    if dropped > 0 {
        eprintln!("process rules: {dropped} rule(s) dropped, target outbound no longer exists");
    }
    // REJECT may be used only by a process rule, so the converter did not
    // necessarily create its built-in block outbound.
    if usable_rules.iter().any(|(_, target)| target == "REJECT")
        && !names.iter().any(|name| name == "REJECT")
    {
        let outbounds = doc
            .get_mut("outbounds")
            .and_then(Json::as_array_mut)
            .ok_or_else(|| {
                serde_json::Error::io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "outbounds must be an array",
                ))
            })?;
        outbounds.push(serde_json::json!({"tag":"REJECT", "type":"block"}));
    }
    if !doc.is_object() || doc.get("route").is_some_and(|route| !route.is_object()) {
        return Err(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "route must be an object",
        )));
    }
    let injected: Vec<Json> = usable_rules
        .iter()
        .map(|(process, target)| {
            serde_json::json!({ "process_name": [process], "outbound": target })
        })
        .collect();
    let existing = doc["route"]["rules"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut merged = injected;
    merged.extend(existing);
    doc["route"]["rules"] = Json::Array(merged);
    serde_json::to_string(&doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(process: &str, target: &str) -> ProcessRule {
        ProcessRule {
            id: format!("r-{process}"),
            process: process.to_string(),
            target: target.to_string(),
        }
    }

    const YAML: &str = "proxies:\n  - name: node\n    type: http\n    server: 127.0.0.1\n    port: 8080\nproxy-groups:\n  - name: auto\n    type: url-test\n    proxies: [node]\nrules:\n  - MATCH,auto\n";

    #[test]
    fn yaml_injects_before_subscription_rules() {
        let out = inject_yaml(YAML, &[rule("curl", "DIRECT"), rule("Telegram", "auto")]).unwrap();
        assert!(out.contains("PROCESS-NAME,curl,DIRECT"));
        assert!(out.contains("PROCESS-NAME,Telegram,auto"));
        // 前插语义：进程规则位于订阅规则（MATCH）之前
        let curl = out.find("PROCESS-NAME,curl").unwrap();
        let telegram = out.find("PROCESS-NAME,Telegram").unwrap();
        let match_rule = out.find("MATCH,auto").unwrap();
        assert!(curl < telegram && telegram < match_rule);
        // 订阅原规则保留
        assert!(out.contains("MATCH,auto"));
    }

    #[test]
    fn yaml_drops_stale_group_targets_and_keeps_direct() {
        // 订阅更新后组名消失：该规则丢弃、DIRECT 规则保留、内核不因未知策略启动失败
        let out = inject_yaml(YAML, &[rule("bank", "已消失的组"), rule("curl", "DIRECT")]).unwrap();
        assert!(!out.contains("已消失的组"));
        assert!(out.contains("PROCESS-NAME,curl,DIRECT"));
        assert!(out.contains("MATCH,auto"));
    }

    #[test]
    fn yaml_creates_rules_section_when_missing_and_noop_when_empty() {
        let out = inject_yaml("proxies: []\n", &[rule("curl", "DIRECT")]).unwrap();
        assert!(out.contains("PROCESS-NAME,curl,DIRECT"));
        // 空规则表：原样返回，不做 roundtrip
        assert_eq!(inject_yaml(YAML, &[]).unwrap(), YAML);
    }

    #[test]
    fn json_injects_process_name_rule_before_subscription() {
        let json = r#"{"outbounds":[{"tag":"auto","type":"url-test"},{"tag":"DIRECT"}],"route":{"rules":[{"inbound":"match-all","outbound":"auto"}]}}"#;
        let out = inject_json(json, &[rule("Telegram", "auto")]).unwrap();
        let doc: Json = serde_json::from_str(&out).unwrap();
        let rules = doc["route"]["rules"].as_array().unwrap();
        assert_eq!(rules[0]["process_name"][0], "Telegram");
        assert_eq!(rules[0]["outbound"], "auto");
        assert_eq!(rules.len(), 2, "订阅原规则保留");
    }

    #[test]
    fn json_drops_stale_outbound_targets() {
        let json = r#"{"outbounds":[{"tag":"auto","type":"url-test"}],"route":{"rules":[]}}"#;
        let out = inject_json(json, &[rule("bank", "gone-group")]).unwrap();
        let doc: Json = serde_json::from_str(&out).unwrap();
        assert!(doc["route"]["rules"].as_array().unwrap().is_empty());
    }
    #[test]
    fn malformed_process_fields_are_not_injected() {
        for process in ["", " ", "curl,DIRECT", "curl\nMATCH,REJECT"] {
            assert_eq!(inject_yaml(YAML, &[rule(process, "DIRECT")]).unwrap(), YAML);
        }
    }
    #[test]
    fn json_process_reject_has_a_declared_outbound() {
        let input = r#"{"outbounds":[{"tag":"DIRECT","type":"direct"}],"route":{"rules":[]}}"#;
        let result: Json =
            serde_json::from_str(&inject_json(input, &[rule("curl", "REJECT")]).unwrap()).unwrap();
        assert_eq!(result["outbounds"][1]["tag"], "REJECT");
        assert_eq!(result["outbounds"][1]["type"], "block");
        assert_eq!(result["route"]["rules"][0]["outbound"], "REJECT");
        let twice: Json = serde_json::from_str(
            &inject_json(&result.to_string(), &[rule("app", "REJECT")]).unwrap(),
        )
        .unwrap();
        assert_eq!(twice["outbounds"].as_array().unwrap().len(), 2);
        assert!(inject_json(
            r#"{"outbounds":[],"route":false}"#,
            &[rule("curl", "DIRECT")]
        )
        .is_err());
    }
}
