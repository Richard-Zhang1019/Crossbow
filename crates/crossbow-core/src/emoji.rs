//! 节点旗帜补全：按节点名中的地区关键词补国旗前缀。
//!
//! 机场订阅常自带旗帜（如 `🇭🇰|香港-中转 01`），本地手写配置一般没有。
//! 本模块把两类配置统一：已有旗帜（区域指示符 emoji）的节点跳过，
//! 其余按关键词匹配并前缀 `国旗|`；同时同步改写 proxy-groups 中的
//! 成员引用，保证配置语义不变。

use serde_yaml::{Mapping, Value};

/// 地区关键词 → 国旗。顺序即优先级，靠前的更具体。
const REGIONS: &[(&str, &[&str], &str)] = &[
    ("香港", &["HK", "Hong Kong", "港"], "🇭🇰"),
    ("台湾", &["TW", "Taiwan", "台"], "🇹🇼"),
    ("日本", &["JP", "Japan", "日"], "🇯🇵"),
    ("韩国", &["KR", "Korea", "韩"], "🇰🇷"),
    ("新加坡", &["SG", "Singapore", "狮城"], "🇸🇬"),
    (
        "美国",
        &["US", "USA", "United States", "洛杉矶", "美"],
        "🇺🇸",
    ),
    ("英国", &["UK", "United Kingdom", "英"], "🇬🇧"),
    ("德国", &["DE", "Germany", "德"], "🇩🇪"),
    ("法国", &["FR", "France", "法"], "🇫🇷"),
    ("加拿大", &["CA", "Canada", "加"], "🇨🇦"),
    ("澳大利亚", &["AU", "Australia", "澳"], "🇦🇺"),
    ("俄罗斯", &["RU", "Russia", "俄"], "🇷🇺"),
    ("印度", &["IN", "India", "印"], "🇮🇳"),
    ("泰国", &["TH", "Thailand", "泰"], "🇹🇭"),
    ("越南", &["VN", "Vietnam", "越"], "🇻🇳"),
    ("菲律宾", &["PH", "Philippines", "菲"], "🇵🇭"),
    ("马来西亚", &["MY", "Malaysia", "马"], "🇲🇾"),
    ("土耳其", &["TR", "Turkey", "土"], "🇹🇷"),
    ("巴西", &["BR", "Brazil", "巴"], "🇧🇷"),
    ("阿根廷", &["AR", "Argentina", "阿"], "🇦🇷"),
    ("荷兰", &["NL", "Netherlands", "荷"], "🇳🇱"),
];

/// 判断字符串是否已含国旗（一对区域指示符 U+1F1E6..U+1F1FF）。
fn has_flag(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    chars.windows(2).any(|w| {
        ('\u{1F1E6}'..='\u{1F1FF}').contains(&w[0]) && ('\u{1F1E6}'..='\u{1F1FF}').contains(&w[1])
    })
}

/// 匹配节点名的地区旗帜；无匹配返回 None。
pub fn flag_for(name: &str) -> Option<&'static str> {
    let lower = name.to_lowercase();
    for (_, keywords, flag) in REGIONS {
        if lower.contains(&keywords[0].to_lowercase()) {
            return Some(flag);
        }
        for kw in &keywords[1..] {
            if lower.contains(&kw.to_lowercase()) {
                return Some(flag);
            }
        }
    }
    None
}

/// 单个名字的补全结果。
pub fn enrich_name(name: &str) -> Option<String> {
    if has_flag(name) {
        return None;
    }
    flag_for(name).map(|f| format!("{f}|{name}"))
}

/// 对整个配置做旗帜补全：重命名 proxies 节点并同步 proxy-groups 引用。
pub fn enrich_flags(config_yaml: &str) -> Result<String, serde_yaml::Error> {
    let mut doc: Value = serde_yaml::from_str(config_yaml)?;
    let Some(proxies) = doc.get_mut("proxies") else {
        return serde_yaml::to_string(&doc);
    };
    let Some(proxy_list) = proxies.as_sequence_mut() else {
        return serde_yaml::to_string(&doc);
    };

    // 旧名 → 新名
    let mut renames: Vec<(String, String)> = Vec::new();
    for p in proxy_list.iter_mut() {
        let Some(name) = p.get("name").and_then(|v| v.as_str()).map(String::from) else {
            continue;
        };
        if let Some(new) = enrich_name(&name) {
            if let Some(pm) = p.as_mapping_mut() {
                pm.insert(Value::from("name"), Value::from(new.clone()));
            }
            renames.push((name, new));
        }
    }
    if renames.is_empty() {
        return serde_yaml::to_string(&doc);
    }

    // 同步 proxy-groups 的成员引用
    if let Some(groups) = doc
        .get_mut("proxy-groups")
        .and_then(|g| g.as_sequence_mut())
    {
        for g in groups.iter_mut() {
            let Some(members) = g.get_mut("proxies").and_then(|m| m.as_sequence_mut()) else {
                continue;
            };
            for m in members.iter_mut() {
                if let Some(s) = m.as_str() {
                    if let Some((_, new)) = renames.iter().find(|(old, _)| old == s) {
                        *m = Value::from(new.clone());
                    }
                }
            }
        }
    }
    serde_yaml::to_string(&doc)
}

/// EngineConfig 的 flag_emoji 字段缺省值。
pub fn default_flag_emoji() -> bool {
    true
}

/// 便于测试/复用的 Mapping 访问。
#[allow(dead_code)]
fn as_mapping(v: &Value) -> Option<&Mapping> {
    v.as_mapping()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_regions() {
        assert_eq!(flag_for("香港-中转 01"), Some("🇭🇰"));
        assert_eq!(flag_for("jp-iepl-02"), Some("🇯🇵"));
        assert_eq!(flag_for("狮城-直连"), Some("🇸🇬"));
        assert_eq!(flag_for("US-01"), Some("🇺🇸"));
        assert_eq!(flag_for("洛杉矶 01"), Some("🇺🇸"));
        assert_eq!(flag_for("剩余流量：1.89 GB"), None);
        assert_eq!(flag_for("套餐到期：长期有效"), None);
    }

    #[test]
    fn skips_existing_flags() {
        assert!(enrich_name("🇭🇰|香港-01").is_none());
        assert_eq!(enrich_name("香港-01").as_deref(), Some("🇭🇰|香港-01"));
    }

    #[test]
    fn enriches_and_rewrites_group_refs() {
        let cfg = r#"
proxies:
  - name: 香港-01
    type: ss
    server: a
    port: 1
  - name: 美国-01
    type: ss
    server: b
    port: 2
proxy-groups:
  - name: 节点选择
    type: select
    proxies:
      - 香港-01
      - 美国-01
      - DIRECT
rules:
  - MATCH,DIRECT
"#;
        let out = enrich_flags(cfg).unwrap();
        assert!(out.contains("🇭🇰|香港-01"), "node renamed: {out}");
        assert!(out.contains("🇺🇸|美国-01"));
        // 组成员引用同步
        let v: Value = serde_yaml::from_str(&out).unwrap();
        let members = v["proxy-groups"][0]["proxies"].as_sequence().unwrap();
        let names: Vec<&str> = members.iter().filter_map(|m| m.as_str()).collect();
        assert_eq!(names, vec!["🇭🇰|香港-01", "🇺🇸|美国-01", "DIRECT"]);
    }

    #[test]
    fn config_without_proxies_passes_through() {
        let out = enrich_flags("port: 7890\n").unwrap();
        assert!(out.contains("port: 7890"));
    }
}
