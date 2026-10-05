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
    for o in active {
        config =
            crate::override_patch::run_override_kind(o.kind, &o.content, &config).map_err(|e| {
                RenderError::OverrideFailed {
                    id: o.id.clone(),
                    name: o.name.clone(),
                    detail: e.to_string(),
                }
            })?;
    }
    if store.engine.flag_emoji {
        config = crate::emoji::enrich_flags(&config)
            .map_err(|e| RenderError::InvalidYaml(e.to_string()))?;
    }
    Ok(Rendered {
        config,
        profile_id: profile_id.to_string(),
    })
}

/// 校验全部启用的覆写：按列表顺序对当前档案内容逐个执行，
/// 返回失败的 (id, 错误)。不要求绑定——编辑即反馈。
pub fn validate_enabled_overrides(store: &StoreSchema) -> Vec<(String, String)> {
    let base = store
        .active_profile
        .as_deref()
        .and_then(|pid| store.profiles.iter().find(|p| p.id == pid))
        .map(|p| p.content.clone())
        .unwrap_or_else(|| "{}".into());
    let mut config = base;
    let mut failures = Vec::new();
    for o in store.overrides.iter().filter(|o| o.enabled) {
        match crate::override_patch::run_override_kind(o.kind, &o.content, &config) {
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
}
