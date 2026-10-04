//! 覆写补丁：YAML 顶层 merge（MVP）。
//!
//! 语义：补丁中的顶层键覆盖基础配置；`rules`/`proxy-groups` 等列表键支持
//! `+key` 前缀表示「前置插入」。JS 脚本覆写在 M1 引入，届时走嵌入 JS runtime。

use serde_yaml::Value;

/// 应用一个 merge 补丁，返回合并后的 YAML 字符串。
pub fn apply_merge(base_yaml: &str, patch_yaml: &str) -> Result<String, serde_yaml::Error> {
    let mut base: Value = serde_yaml::from_str(base_yaml)?;
    let patch: Value = serde_yaml::from_str(patch_yaml)?;
    merge_into(&mut base, &patch);
    serde_yaml::to_string(&base)
}

/// 应用覆写链（按序）。
pub fn apply_chain(base_yaml: &str, patches: &[String]) -> Result<String, serde_yaml::Error> {
    let mut cur = base_yaml.to_string();
    for p in patches {
        cur = apply_merge(&cur, p)?;
    }
    Ok(cur)
}

fn merge_into(base: &mut Value, patch: &Value) {
    match patch {
        Value::Mapping(map) => {
            if !matches!(base, Value::Mapping(_)) {
                *base = Value::Mapping(Default::default());
            }
            let bmap = base.as_mapping_mut().unwrap();
            for (k, v) in map {
                let key_str = k.as_str().unwrap_or_default();
                if let Some(list_key) = key_str.strip_prefix('+') {
                    // `+rules`：补丁列表前置插入到基础列表前。
                    let real = Value::String(list_key.to_string());
                    let prepend = to_list(v);
                    let existing = bmap.get(&real).map(to_list).unwrap_or_default();
                    let mut merged = prepend;
                    merged.extend(existing);
                    bmap.insert(real, Value::Sequence(merged));
                } else if v.is_mapping() {
                    let child = bmap
                        .entry(k.clone())
                        .or_insert(Value::Mapping(Default::default()));
                    merge_into(child, v);
                } else {
                    bmap.insert(k.clone(), v.clone());
                }
            }
        }
        _ => *base = patch.clone(),
    }
}

fn to_list(v: &Value) -> Vec<Value> {
    match v {
        Value::Sequence(s) => s.clone(),
        Value::Null => Vec::new(),
        other => vec![other.clone()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_top_level_scalar() {
        let out = apply_merge("port: 7890\nmode: rule\n", "port: 7897\n").unwrap();
        assert!(out.contains("port: 7897"));
        assert!(out.contains("mode: rule"));
    }

    #[test]
    fn plus_key_prepends_rules() {
        let base = "rules:\n  - MATCH,DIRECT\n";
        let patch = "+rules:\n  - DOMAIN,ads.example.com,REJECT\n";
        let out = apply_merge(base, patch).unwrap();
        let ads_pos = out.find("ads.example.com").unwrap();
        let match_pos = out.find("MATCH,DIRECT").unwrap();
        assert!(ads_pos < match_pos, "prepended rule should come first");
    }

    #[test]
    fn chain_applies_in_order() {
        let out = apply_chain("port: 1\n", &["port: 2\n".into(), "port: 3\n".into()]).unwrap();
        assert!(out.contains("port: 3"));
    }
}
