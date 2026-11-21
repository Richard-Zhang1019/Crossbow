//! 配置渲染：Profile 原文 + 覆写链 → 交给内核的最终配置。
//!
//! MVP 目标内核是 mihomo（YAML 进 YAML 出）；M1 加 sing-box 时在这里
//! 增加 YAML → JSON 的转换 target（见 DESIGN §3.3）。

use crate::model::StoreSchema;

pub struct Rendered {
    /// 最终交给内核的配置文本。
    pub config: String,
    /// 实际生效的 profile id。
    pub profile_id: String,
}

/// 渲染当前激活 Profile。无激活 Profile 或内容为空时返回错误，
/// 由壳层决定是停用内核还是回落直连。
pub fn render_config(store: &StoreSchema) -> Result<Rendered, RenderError> {
    render_config_with(store, true)
}

/// `use_overrides=false` 为安全模式：跳过覆写链（坏覆写导致内核启动
/// 失败时的自动降级路径）。
pub fn render_config_with(
    store: &StoreSchema,
    use_overrides: bool,
) -> Result<Rendered, RenderError> {
    let profile_id = store
        .active_profile
        .as_deref()
        .ok_or(RenderError::NoActiveProfile)?;
    let profile = store
        .profiles
        .iter()
        .find(|p| p.id == profile_id)
        .ok_or_else(|| RenderError::MissingProfile(profile_id.to_string()))?;

    let active: Vec<&crate::OverrideDef> = if use_overrides {
        store
            .overrides
            .iter()
            .filter(|o| o.enabled && profile.override_ids.contains(&o.id))
            .collect()
    } else {
        Vec::new()
    };

    let mut config = profile.content.clone();
    let deadline = std::time::Instant::now() + crate::script_override::WORKER_TIMEOUT;
    for o in active {
        config = crate::override_patch::run_override_before(o.kind, &o.content, &config, deadline)
            .map_err(|e| RenderError::OverrideFailed {
                id: o.id.clone(),
                name: o.name.clone(),
                detail: e.to_string(),
            })?;
    }
    if store.engine.flag_emoji {
        config = crate::emoji::enrich_flags(&config)
            .map_err(|e| RenderError::InvalidYaml(e.to_string()))?;
    }
    // 按进程分流规则（#570）：前插到订阅规则之前；不参与覆写链
    // （安全模式跳过覆写链时进程规则依然生效），目标策略失效的规则
    // 防御性丢弃，避免订阅更新后内核因未知策略启动失败。
    let config = match store.engine.engine {
        crate::model::Engine::SingBox => {
            // Native sing-box profiles may already have been round-tripped to
            // YAML by overrides/flag enrichment. Preserve their schema rather
            // than sending them through the Clash-only converter.
            let document: serde_json::Value = serde_yaml::from_str(&config)
                .map_err(|_| RenderError::InvalidYaml("配置语法无效".into()))?;
            let json = if document.get("proxies").is_none()
                && document.get("outbounds").is_some_and(|v| v.is_array())
            {
                serde_json::to_string(&document)
                    .map_err(|e| RenderError::InvalidYaml(e.to_string()))?
            } else {
                crate::singbox_convert::convert_to_singbox(
                    &config,
                    store.engine.mixed_port,
                    store.engine.allow_lan,
                )
                .map_err(|e| RenderError::InvalidYaml(e.to_string()))?
            };
            let json = crate::rule_sets::inject_json(&json, &store.rule_sets)
                .map_err(RenderError::InvalidYaml)?;
            crate::process_rules::inject_json(&json, &store.process_rules)
                .map_err(|e| RenderError::InvalidYaml(e.to_string()))?
        }
        crate::model::Engine::Mihomo => {
            let config = crate::rule_sets::inject_yaml(&config, &store.rule_sets)
                .map_err(RenderError::InvalidYaml)?;
            crate::process_rules::inject_yaml(&config, &store.process_rules)
                .map_err(|e| RenderError::InvalidYaml(e.to_string()))?
        }
    };
    Ok(Rendered {
        config,
        profile_id: profile_id.to_string(),
    })
}

/// 校验全部启用的覆写：按列表顺序对当前档案内容逐个执行，
/// 返回失败的 (id, 错误)。不要求绑定——编辑即反馈。
pub fn validate_enabled_overrides(store: &StoreSchema) -> Vec<(String, String)> {
    validate_with_budget(store, crate::script_override::WORKER_TIMEOUT)
}

fn validate_with_budget(store: &StoreSchema, budget: std::time::Duration) -> Vec<(String, String)> {
    let deadline = std::time::Instant::now() + budget;
    let base = store
        .active_profile
        .as_deref()
        .and_then(|pid| store.profiles.iter().find(|p| p.id == pid))
        .map(|p| p.content.clone())
        .unwrap_or_else(|| "{}".into());
    let mut config = base;
    let mut failures = Vec::new();
    for o in store.overrides.iter().filter(|o| o.enabled) {
        if std::time::Instant::now() >= deadline {
            failures.push((
                o.id.clone(),
                "未校验：本次覆写处理预算已耗尽，请减少脚本或停用慢脚本后重试".into(),
            ));
            continue;
        }
        match crate::override_patch::run_override_before(o.kind, &o.content, &config, deadline) {
            Ok(next) => config = next,
            Err(e) => failures.push((o.id.clone(), e.to_string())),
        }
    }
    failures
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("覆写「{name}」执行失败：{detail}")]
    OverrideFailed {
        id: String,
        name: String,
        detail: String,
    },
    #[error("no active profile")]
    NoActiveProfile,
    #[error("active profile {0} missing")]
    MissingProfile(String),
    #[error("invalid yaml: {0}")]
    InvalidYaml(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{OverrideDef, OverrideKind, Profile, ProfileKind};

    #[test]
    fn validation_deadline_marks_remaining_items_unchecked() {
        let mut store = fixture();
        store.overrides.clear();
        for (id, content) in [
            (
                "slow",
                "function main(c) { /(a+)+$/.test('a'.repeat(36)+'!'); return c; }",
            ),
            ("later", "function main(c) { return c; }"),
        ] {
            store.overrides.push(OverrideDef {
                id: id.into(),
                name: id.into(),
                kind: OverrideKind::Script,
                content: content.into(),
                enabled: true,
                last_error: None,
            });
        }
        let started = std::time::Instant::now();
        let failures = validate_with_budget(&store, std::time::Duration::from_millis(150));
        assert_eq!(failures.len(), 2);
        assert!(failures[0].1.contains("超时"));
        assert!(failures[1].1.contains("未校验"));
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
    }

    #[test]
    fn validate_flags_all_enabled_failures() {
        let mut s = fixture();
        s.overrides.push(OverrideDef {
            id: "bad".into(),
            name: "bad".into(),
            kind: OverrideKind::Script,
            enabled: true,
            content: "function main(c) { return c.nope.total; }".into(),
            last_error: None,
        });
        let fails = validate_enabled_overrides(&s);
        assert_eq!(fails.len(), 1);
        assert_eq!(fails[0].0, "bad");
    }

    #[test]
    fn validate_skips_disabled() {
        let mut s = fixture();
        s.overrides.push(OverrideDef {
            id: "bad-off".into(),
            name: "bad-off".into(),
            kind: OverrideKind::Script,
            enabled: false,
            content: "function main(c) { return c.nope.total; }".into(),
            last_error: None,
        });
        assert!(validate_enabled_overrides(&s).is_empty());
    }

    fn fixture() -> StoreSchema {
        let mut s = StoreSchema::default();
        s.profiles.push(Profile {
            id: "p1".into(),
            name: "sub-a".into(),
            kind: ProfileKind::Remote,
            url: None,
            content: "mixed-port: 7890\nrules:\n  - MATCH,DIRECT\n".into(),
            update_interval_min: 360,
            override_ids: vec!["o1".into()],
            last_updated: None,
            traffic: None,
            last_update: None,
        });
        s.overrides.push(OverrideDef {
            id: "o1".into(),
            name: "port-fix".into(),
            kind: OverrideKind::Merge,
            enabled: true,
            content: "mixed-port: 7897\n".into(),
            last_error: None,
        });
        s.active_profile = Some("p1".into());
        s
    }

    #[test]
    fn renders_profile_with_enabled_overrides() {
        let rendered = render_config(&fixture()).unwrap();
        assert!(rendered.config.contains("mixed-port: 7897"));
        assert!(rendered.config.contains("MATCH,DIRECT"));
    }

    #[test]
    fn disabled_override_is_skipped() {
        let mut s = fixture();
        s.overrides[0].enabled = false;
        let rendered = render_config(&s).unwrap();
        assert!(rendered.config.contains("mixed-port: 7890"));
    }

    #[test]
    fn no_active_profile_errors() {
        let mut s = fixture();
        s.active_profile = None;
        assert!(matches!(
            render_config(&s),
            Err(RenderError::NoActiveProfile)
        ));
    }

    fn singbox_fixture(tolerance: &str) -> StoreSchema {
        let mut s = fixture();
        s.engine.engine = crate::model::Engine::SingBox;
        s.profiles[0].content = format!(
            "proxies:\n  - name: node\n    type: http\n    server: 127.0.0.1\n    port: 8080\nproxy-groups:\n  - name: auto\n    type: url-test\n    proxies: [node]\n{tolerance}rules:\n  - MATCH,auto\n"
        );
        s
    }

    #[test]
    fn singbox_tolerance_errors_reach_render_preflight() {
        for tolerance in ["", "    tolerance: 0\n", "    tolerance: 65536\n"] {
            let s = singbox_fixture(tolerance);
            for use_overrides in [true, false] {
                assert!(matches!(
                    render_config_with(&s, use_overrides),
                    Err(RenderError::InvalidYaml(m))
                        if m.contains("auto") && m.contains("tolerance")
                ));
            }
        }
    }

    #[test]
    fn singbox_tolerance_validation_uses_final_override_result() {
        let mut s = singbox_fixture("    tolerance: 0\n");
        s.overrides[0].content = "proxy-groups:\n  - name: auto\n    type: url-test\n    proxies: [node]\n    tolerance: 120\n".into();
        let rendered = render_config(&s).unwrap();
        let config: serde_json::Value = serde_json::from_str(&rendered.config).unwrap();
        let group = config["outbounds"]
            .as_array()
            .unwrap()
            .iter()
            .find(|outbound| outbound["tag"] == "auto")
            .unwrap();
        assert_eq!(group["tolerance"], 120);
        // 安全模式跳过覆写后，原配置的零容差仍须拒绝，不能使用内核默认值。
        assert!(matches!(
            render_config_with(&s, false),
            Err(RenderError::InvalidYaml(m)) if m.contains("50ms")
        ));
    }

    #[test]
    fn singbox_preflight_rejects_rules_broken_by_an_override() {
        let mut s = singbox_fixture("    tolerance: 120\n");
        s.overrides[0].kind = OverrideKind::Script;
        s.overrides[0].content = "function main(c) { c.rules.push(false); return c; }".into();

        assert!(matches!(
            render_config(&s),
            Err(RenderError::InvalidYaml(message))
                if message.contains("rules 第 2 条") && message.contains("bool")
        ));
        assert!(render_config_with(&s, false).is_ok());

        s.overrides[0].content = "function main(c) { return c; }".into();
        assert!(render_config(&s).is_ok());
    }

    #[test]
    fn singbox_group_fields_follow_overrides_and_node_renames() {
        let mut s = fixture();
        s.engine.engine = crate::model::Engine::SingBox;
        s.engine.flag_emoji = true;
        s.profiles[0].content = r#"
proxies:
  - {name: '🇨🇦|local', type: http, server: 127.0.0.1, port: 8080}
  - {name: 香港, type: http, server: 127.0.0.1, port: 8081}
proxy-groups:
  - {name: manual, type: select, include-all-proxies: true, exclude-filter: 香港}
rules: ['MATCH,manual']
"#
        .into();
        s.overrides[0].content = r#"
proxy-groups:
  - {name: manual, type: select, include-all-proxies: true, default-selected: 香港}
"#
        .into();

        let rendered = render_config(&s).unwrap();
        let config: serde_json::Value = serde_json::from_str(&rendered.config).unwrap();
        let group = config["outbounds"]
            .as_array()
            .unwrap()
            .iter()
            .find(|outbound| outbound["tag"] == "manual")
            .unwrap();
        assert_eq!(
            group["outbounds"],
            serde_json::json!(["🇨🇦|local", "🇭🇰|香港"])
        );
        assert_eq!(group["default"], "🇭🇰|香港");
        assert!(matches!(
            render_config_with(&s, false),
            Err(RenderError::InvalidYaml(m))
                if m.contains("manual") && m.contains("exclude-filter") && m.contains("香港")
        ));
    }
}
