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
    let profile_id = store
        .active_profile
        .as_deref()
        .ok_or(RenderError::NoActiveProfile)?;
    let profile = store
        .profiles
        .iter()
        .find(|p| p.id == profile_id)
        .ok_or_else(|| RenderError::MissingProfile(profile_id.to_string()))?;

    let active: Vec<&crate::OverrideDef> = store
        .overrides
        .iter()
        .filter(|o| o.enabled && profile.override_ids.contains(&o.id))
        .collect();

    let mut config = profile.content.clone();
    for o in active {
        config = crate::override_patch::run_override_kind(o.kind, &o.content, &config)
            .map_err(|e| RenderError::InvalidYaml(e.to_string()))?;
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

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
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
