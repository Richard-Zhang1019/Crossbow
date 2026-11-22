//! Versioned local migration of profiles, overrides and the active selection.
//! Package contents can contain credentials. Errors and previews use fixed
//! categories and counts only.

use crate::model::{OverrideDef, OverrideKind, Profile, StoreSchema, SCHEMA_VERSION};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const PACKAGE_VERSION: u32 = 2;
pub const MAX_PACKAGE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct ConfigPackage {
    package_version: u32,
    #[serde(default)]
    created_at: u64,
    schema_version: u32,
    payload: ConfigPayload,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ConfigPayload {
    profiles: Vec<Profile>,
    overrides: Vec<OverrideDef>,
    #[serde(default)]
    active_profile: Option<String>,
    #[serde(default)]
    process_rules: Vec<crate::ProcessRule>,
    #[serde(default)]
    rule_sets: Vec<crate::rule_sets::RuleSet>,
}

#[derive(Debug, Serialize)]
pub struct ConfigImportReport {
    pub added_profiles: usize,
    pub replaced_profiles: usize,
    pub added_overrides: usize,
    pub replaced_overrides: usize,
    pub selects_profile: bool,
    pub added_process_rules: usize,
    pub replaced_process_rules: usize,
    pub added_rule_sets: usize,
    pub replaced_rule_sets: usize,
    pub policy_conflicts: usize,
    pub same_name_items: usize,
    pub unverified_targets: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    #[error("pkg.errorSize")]
    Size,
    #[error("pkg.errorFormat")]
    Format,
    #[error("pkg.errorVersion")]
    Version,
    #[error("pkg.errorSchema")]
    Schema,
    #[error("pkg.errorEmpty")]
    Empty,
    #[error("pkg.errorProfiles")]
    Profiles,
    #[error("pkg.errorOverrides")]
    Overrides,
    #[error("pkg.errorSelection")]
    Selection,
    #[error("pkg.errorBindings")]
    Bindings,
    #[error("pkg.errorSharedOverride")]
    SharedOverride,
    #[error("pkg.errorPolicies")]
    Policies,
}

pub fn export_package(data: &StoreSchema) -> Result<String, PackageError> {
    let mut payload = ConfigPayload {
        profiles: data.profiles.clone(),
        overrides: data.overrides.clone(),
        active_profile: data.active_profile.clone(),
        process_rules: data.process_rules.clone(),
        rule_sets: data.rule_sets.clone(),
    };
    payload.clear_diagnostics();
    payload.validate()?;
    let package = ConfigPackage {
        package_version: PACKAGE_VERSION,
        created_at: crate::subscription::now_secs(),
        schema_version: data.schema_version,
        payload,
    };
    let text = serde_json::to_string_pretty(&package).map_err(|_| PackageError::Format)?;
    if text.len() > MAX_PACKAGE_BYTES {
        return Err(PackageError::Size);
    }
    Ok(text)
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}
fn valid_target(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= 256
        && !value.chars().any(|c| c == ',' || c.is_control())
}
fn merge_items<T>(current: &mut Vec<T>, incoming: Vec<T>, id: impl Fn(&T) -> &str) {
    let ids: BTreeSet<String> = incoming.iter().map(|v| id(v).to_owned()).collect();
    current.retain(|v| !ids.contains(id(v)));
    current.extend(incoming);
}

impl ConfigPayload {
    pub fn parse(text: &[u8]) -> Result<Self, PackageError> {
        if text.len() > MAX_PACKAGE_BYTES {
            return Err(PackageError::Size);
        }
        let package: ConfigPackage =
            serde_json::from_slice(text).map_err(|_| PackageError::Format)?;
        if !(1..=PACKAGE_VERSION).contains(&package.package_version) {
            return Err(PackageError::Version);
        }
        if package.schema_version > SCHEMA_VERSION {
            return Err(PackageError::Schema);
        }
        // Older supported schemas share these portable fields. Serde fills in
        // optional fields before we validate references and drop diagnostics.
        if package.package_version == 1
            && (!package.payload.process_rules.is_empty() || !package.payload.rule_sets.is_empty())
        {
            return Err(PackageError::Version);
        }
        let mut payload = package.payload;
        payload.clear_diagnostics();
        payload.validate()?;
        Ok(payload)
    }

    fn clear_diagnostics(&mut self) {
        for set in &mut self.rule_sets {
            set.last_error = None;
            set.updated_at = 0;
        }

        for profile in &mut self.profiles {
            profile.last_update = None;
        }
        for override_def in &mut self.overrides {
            override_def.last_error = None;
        }
    }

    fn validate(&self) -> Result<(), PackageError> {
        if self.profiles.is_empty()
            && self.overrides.is_empty()
            && self.process_rules.is_empty()
            && self.rule_sets.is_empty()
        {
            return Err(PackageError::Empty);
        }
        if self.process_rules.len() > 512 || self.rule_sets.len() > crate::rule_sets::MAX_SETS {
            return Err(PackageError::Policies);
        }
        let mut ids = BTreeSet::new();
        let mut processes = BTreeSet::new();
        for rule in &self.process_rules {
            if !valid_id(&rule.id)
                || !ids.insert(&rule.id)
                || !processes.insert(&rule.process)
                || !crate::process_rules::valid_process_name(&rule.process)
                || !valid_target(&rule.target)
            {
                return Err(PackageError::Policies);
            }
        }
        ids.clear();
        let mut names = BTreeSet::new();
        for set in &self.rule_sets {
            if !valid_id(&set.id)
                || set.name.trim().is_empty()
                || set.name.chars().count() > 80
                || set.name.chars().any(char::is_control)
                || !(300..=604800).contains(&set.interval_secs)
                || !ids.insert(&set.id)
                || !names.insert(&set.name)
                || !valid_target(&set.target)
                || crate::rule_sets::canonical_url(&set.url).is_err()
                || crate::rule_sets::parse_payload(
                    &serde_json::json!({"payload":set.payload}).to_string(),
                    set.behavior,
                )
                .is_err()
            {
                return Err(PackageError::Policies);
            }
        }
        let mut override_ids = BTreeSet::new();
        for item in &self.overrides {
            if item.id.trim().is_empty()
                || item.name.trim().is_empty()
                || !override_ids.insert(item.id.as_str())
            {
                return Err(PackageError::Overrides);
            }
            if item.enabled
                && item.kind == OverrideKind::Merge
                && !serde_yaml::from_str::<serde_yaml::Value>(&item.content)
                    .is_ok_and(|value| value.is_mapping())
            {
                return Err(PackageError::Overrides);
            }
        }
        let mut profile_ids = BTreeSet::new();
        for profile in &self.profiles {
            if profile.id.trim().is_empty()
                || profile.name.trim().is_empty()
                || !profile_ids.insert(profile.id.as_str())
                || crate::subscription::validate_profile_content(&profile.content).is_err()
            {
                return Err(PackageError::Profiles);
            }
            let mut bindings = BTreeSet::new();
            if profile
                .override_ids
                .iter()
                .any(|id| !override_ids.contains(id.as_str()) || !bindings.insert(id.as_str()))
            {
                return Err(PackageError::Bindings);
            }
        }
        if self
            .active_profile
            .as_deref()
            .is_some_and(|id| !profile_ids.contains(id))
        {
            return Err(PackageError::Selection);
        }
        Ok(())
    }

    pub fn preview(&self, current: &StoreSchema) -> Result<ConfigImportReport, PackageError> {
        let profile_ids: BTreeSet<_> = self.profiles.iter().map(|p| p.id.as_str()).collect();
        let override_ids: BTreeSet<_> = self.overrides.iter().map(|o| o.id.as_str()).collect();
        // The order in StoreSchema.overrides is the execution order. Replacing
        // or moving a shared override could change a profile outside the package.
        if current.profiles.iter().any(|p| {
            !profile_ids.contains(p.id.as_str())
                && p.override_ids
                    .iter()
                    .any(|id| override_ids.contains(id.as_str()))
        }) {
            return Err(PackageError::SharedOverride);
        }
        let replaced_profiles = current
            .profiles
            .iter()
            .filter(|p| profile_ids.contains(p.id.as_str()))
            .count();
        let replaced_overrides = current
            .overrides
            .iter()
            .filter(|o| override_ids.contains(o.id.as_str()))
            .count();
        let incoming_process_ids: BTreeSet<_> =
            self.process_rules.iter().map(|r| r.id.as_str()).collect();
        let incoming_set_ids: BTreeSet<_> = self.rule_sets.iter().map(|r| r.id.as_str()).collect();
        let replaced_process_rules = current
            .process_rules
            .iter()
            .filter(|r| incoming_process_ids.contains(r.id.as_str()))
            .count();
        let replaced_rule_sets = current
            .rule_sets
            .iter()
            .filter(|r| incoming_set_ids.contains(r.id.as_str()))
            .count();
        let mut process_names = BTreeSet::new();
        let mut set_names = BTreeSet::new();
        let mut policy_conflicts = 0;
        for r in current
            .process_rules
            .iter()
            .filter(|r| !incoming_process_ids.contains(r.id.as_str()))
            .chain(self.process_rules.iter())
        {
            if !process_names.insert(&r.process) {
                policy_conflicts += 1;
            }
        }
        for r in current
            .rule_sets
            .iter()
            .filter(|r| !incoming_set_ids.contains(r.id.as_str()))
            .chain(self.rule_sets.iter())
        {
            if !set_names.insert(&r.name) {
                policy_conflicts += 1;
            }
        }
        let mut names = BTreeSet::new();
        let same_profiles = current
            .profiles
            .iter()
            .filter(|p| !profile_ids.contains(p.id.as_str()))
            .chain(self.profiles.iter())
            .filter(|p| !names.insert(p.name.as_str()))
            .count();
        let mut names = BTreeSet::new();
        let same_overrides = current
            .overrides
            .iter()
            .filter(|o| !override_ids.contains(o.id.as_str()))
            .chain(self.overrides.iter())
            .filter(|o| !names.insert(o.name.as_str()))
            .count();
        let same_name_items = same_profiles + same_overrides;
        // Static preview only: never execute overrides or download providers here.
        let selected = self
            .active_profile
            .as_ref()
            .or(current.active_profile.as_ref());
        let selected = selected.and_then(|id| {
            self.profiles
                .iter()
                .chain(current.profiles.iter())
                .find(|p| &p.id == id)
        });
        let targets = selected
            .and_then(|p| serde_yaml::from_str::<serde_json::Value>(&p.content).ok())
            .map(|d| crate::rule_sets::targets(&d))
            .unwrap_or_default();
        let unverified_targets = self
            .process_rules
            .iter()
            .map(|r| &r.target)
            .chain(
                self.rule_sets
                    .iter()
                    .filter(|r| r.enabled)
                    .map(|r| &r.target),
            )
            .filter(|t| !matches!(t.as_str(), "DIRECT" | "REJECT") && !targets.contains(t))
            .count();
        Ok(ConfigImportReport {
            added_process_rules: self
                .process_rules
                .len()
                .saturating_sub(replaced_process_rules),
            replaced_process_rules,
            added_rule_sets: self.rule_sets.len().saturating_sub(replaced_rule_sets),
            replaced_rule_sets,
            policy_conflicts,
            same_name_items,
            unverified_targets,
            added_profiles: self.profiles.len().saturating_sub(replaced_profiles),
            replaced_profiles,
            added_overrides: self.overrides.len().saturating_sub(replaced_overrides),
            replaced_overrides,
            selects_profile: self.active_profile.is_some(),
        })
    }

    pub fn validate_targets(&self, rendered: &str) -> Result<(), PackageError> {
        let document: serde_json::Value =
            serde_yaml::from_str(rendered).map_err(|_| PackageError::Policies)?;
        let targets = crate::rule_sets::targets(&document);
        let process_targets: BTreeSet<&str> = [("proxy-groups", "name"), ("outbounds", "tag")]
            .into_iter()
            .flat_map(|(section, key)| {
                document[section]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(move |v| v[key].as_str())
            })
            .chain(["DIRECT", "REJECT"])
            .collect();
        if self
            .process_rules
            .iter()
            .any(|r| !process_targets.contains(r.target.as_str()))
            || self
                .rule_sets
                .iter()
                .filter(|r| r.enabled)
                .any(|r| !targets.contains(&r.target))
        {
            return Err(PackageError::Policies);
        }
        Ok(())
    }
    pub fn changes_active_profile(&self, current: &StoreSchema) -> bool {
        !self.process_rules.is_empty()
            || !self.rule_sets.is_empty()
            || self.active_profile.is_some()
            || current
                .active_profile
                .as_deref()
                .is_some_and(|id| self.profiles.iter().any(|profile| profile.id == id))
    }

    pub fn merge_into(self, current: &mut StoreSchema) -> Result<ConfigImportReport, PackageError> {
        let report = self.preview(current)?;
        if report.policy_conflicts > 0
            || current.process_rules.len() + report.added_process_rules > 512
            || current.rule_sets.len() + report.added_rule_sets > crate::rule_sets::MAX_SETS
        {
            return Err(PackageError::Policies);
        }

        let profile_ids: BTreeSet<_> = self.profiles.iter().map(|p| p.id.as_str()).collect();
        let override_ids: BTreeSet<_> = self.overrides.iter().map(|o| o.id.as_str()).collect();
        current
            .profiles
            .retain(|p| !profile_ids.contains(p.id.as_str()));
        current
            .overrides
            .retain(|o| !override_ids.contains(o.id.as_str()));
        merge_items(&mut current.process_rules, self.process_rules, |r| &r.id);
        let mut sets = self.rule_sets;
        for set in &mut sets {
            set.updated_at = crate::subscription::now_secs();
        }
        merge_items(&mut current.rule_sets, sets, |r| &r.id);
        current.profiles.extend(self.profiles);
        current.overrides.extend(self.overrides);
        if self.active_profile.is_some() {
            current.active_profile = self.active_profile;
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ProfileKind, UpdateOutcome};
    use crate::Store;

    fn profile(id: &str, bindings: &[&str]) -> Profile {
        Profile {
            id: id.into(),
            name: id.into(),
            kind: ProfileKind::Local,
            url: None,
            content: "proxies: []\nrules: ['MATCH,DIRECT']".into(),
            update_interval_min: 0,
            override_ids: bindings.iter().map(ToString::to_string).collect(),
            last_updated: None,
            traffic: None,
            last_update: None,
        }
    }

    fn patch(id: &str, port: u16, enabled: bool) -> OverrideDef {
        OverrideDef {
            id: id.into(),
            name: id.into(),
            kind: OverrideKind::Merge,
            enabled,
            content: format!("mixed-port: {port}"),
            last_error: None,
        }
    }

    fn fixture() -> StoreSchema {
        StoreSchema {
            profiles: vec![
                profile("primary", &["a", "b", "off"]),
                profile("secondary", &[]),
            ],
            overrides: vec![
                patch("a", 1001, true),
                patch("b", 1002, true),
                patch("off", 1003, false),
            ],
            active_profile: Some("primary".into()),
            ..StoreSchema::default()
        }
    }

    #[test]
    fn restores_into_clean_store_and_reopens_with_binding_order() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let exported = export_package(&fixture()).unwrap();
        let payload = ConfigPayload::parse(exported.as_bytes()).unwrap();
        let report = payload.merge_into(store.data_mut()).unwrap();
        assert_eq!((report.added_profiles, report.added_overrides), (2, 3));
        store.save().unwrap();

        let reopened = Store::open(dir.path()).unwrap();
        let data = reopened.data();
        assert_eq!(data.active_profile.as_deref(), Some("primary"));
        assert_eq!(data.profiles[0].override_ids, ["a", "b", "off"]);
        assert!(!data.overrides[2].enabled);
        assert!(crate::render_config(data)
            .unwrap()
            .config
            .contains("mixed-port: 1002"));
    }

    #[test]
    fn preserves_unrelated_entries_and_machine_settings_with_recoverable_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let data = store.data_mut();
        data.engine.mixed_port = 9876;
        data.engine.tun_enable = true;
        data.ui.lang = "en".into();
        data.profiles = vec![
            profile("local-only", &["local-patch"]),
            profile("primary", &[]),
        ];
        data.overrides = vec![
            patch("b", 1002, true),
            patch("a", 1001, true),
            patch("local-patch", 1004, true),
        ];
        data.active_profile = Some("local-only".into());
        store.save().unwrap();

        let mut source = fixture();
        source.active_profile = None;
        let text = export_package(&source).unwrap();
        let payload = ConfigPayload::parse(text.as_bytes()).unwrap();
        let report = payload.merge_into(store.data_mut()).unwrap();
        assert_eq!((report.added_profiles, report.replaced_profiles), (1, 1));
        assert_eq!((report.added_overrides, report.replaced_overrides), (1, 2));
        store.save().unwrap();

        let store = Store::open(dir.path()).unwrap();
        let data = store.data();
        assert_eq!(data.profiles.len(), 3);
        assert_eq!(data.profiles[0].override_ids, ["local-patch"]);
        assert_eq!(
            data.overrides
                .iter()
                .map(|o| o.id.as_str())
                .collect::<Vec<_>>(),
            ["local-patch", "a", "b", "off"]
        );
        assert_eq!(data.active_profile.as_deref(), Some("local-only"));
        assert_eq!(data.engine.mixed_port, 9876);
        assert!(data.engine.tun_enable);
        assert_eq!(data.ui.lang, "en");

        let snapshots = store.snapshots().unwrap();
        let id = snapshots[0].file_name().and_then(|n| n.to_str()).unwrap();
        let before = store.snapshot_data(id).unwrap();
        assert_eq!(before.profiles.len(), 2);
        assert_eq!(before.overrides[0].id, "b");
    }

    #[test]
    fn shared_override_conflict_does_not_mutate_store() {
        let mut target = StoreSchema {
            profiles: vec![profile("keep", &["a"])],
            overrides: vec![patch("a", 9999, true)],
            ..StoreSchema::default()
        };
        let text = export_package(&fixture()).unwrap();
        let payload = ConfigPayload::parse(text.as_bytes()).unwrap();
        let before = serde_json::to_vec(&target).unwrap();
        assert!(matches!(
            payload.merge_into(&mut target),
            Err(PackageError::SharedOverride)
        ));
        assert_eq!(serde_json::to_vec(&target).unwrap(), before);
    }

    #[test]
    fn validates_versions_references_duplicates_and_size() {
        let text = export_package(&fixture()).unwrap();
        let valid: serde_json::Value = serde_json::from_str(&text).unwrap();
        for (path, value, check) in [
            (
                "/package_version",
                serde_json::json!(99),
                "pkg.errorVersion",
            ),
            ("/schema_version", serde_json::json!(99), "pkg.errorSchema"),
            (
                "/payload/active_profile",
                serde_json::json!("secret-marker"),
                "pkg.errorSelection",
            ),
            (
                "/payload/profiles/0/override_ids",
                serde_json::json!(["foreign"]),
                "pkg.errorBindings",
            ),
            (
                "/payload/profiles/1/id",
                serde_json::json!("primary"),
                "pkg.errorProfiles",
            ),
            (
                "/payload/overrides/1/id",
                serde_json::json!("a"),
                "pkg.errorOverrides",
            ),
            (
                "/payload/profiles/1/content",
                serde_json::json!("invalid-secret: ["),
                "pkg.errorProfiles",
            ),
        ] {
            let mut changed = valid.clone();
            *changed.pointer_mut(path).unwrap() = value;
            let outcome = ConfigPayload::parse(&serde_json::to_vec(&changed).unwrap());
            assert!(matches!(outcome, Err(error) if error.to_string() == check));
        }
        let mut legacy = valid;
        legacy["schema_version"] = serde_json::json!(0);
        assert!(ConfigPayload::parse(&serde_json::to_vec(&legacy).unwrap()).is_ok());
        assert!(matches!(
            ConfigPayload::parse(b"{"),
            Err(PackageError::Format)
        ));
        assert!(matches!(
            ConfigPayload::parse(&vec![b'a'; MAX_PACKAGE_BYTES + 1]),
            Err(PackageError::Size)
        ));
    }

    #[test]
    fn sample_package_keeps_original_store_on_write_failure() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store.data_mut().profiles.push(profile("previous", &[]));
        store.data_mut().active_profile = Some("previous".into());
        store.save().unwrap();
        let payload =
            ConfigPayload::parse(include_bytes!("../../../docs/config-package/example.json"))
                .unwrap();
        let report = payload.merge_into(store.data_mut()).unwrap();
        assert_eq!((report.added_profiles, report.added_overrides), (2, 2));
        assert_eq!(store.data().active_profile.as_deref(), Some("sample-local"));
        // #596 后写入走随机独占临时文件——注入改用 secure_write 的
        // 仅主文件的 partial-write 阶段注入失败，验证成功快照与旧库保留。
        let _inject = crate::secure_write::inject_path_at(&dir.path().join("store.json"), 2);
        let save_result = store.save();
        assert!(save_result.is_err());

        let reopened = Store::open(dir.path()).unwrap();
        assert_eq!(reopened.data().active_profile.as_deref(), Some("previous"));
        assert_eq!(reopened.data().profiles.len(), 1);
        // The snapshot completes; the primary file's partial write fails.
        assert_eq!(reopened.snapshots().unwrap().len(), 1);
    }

    #[test]
    fn drops_runtime_diagnostics_but_preserves_portable_content() {
        let mut store = fixture();
        store.profiles[0].last_update = Some(UpdateOutcome {
            ok: false,
            changes: None,
            at: 1,
            detail: Some("private-diagnostic".into()),
        });
        store.profiles[0].url = Some("https://example.invalid/sub?token=portable-secret".into());
        store.overrides[0].last_error = Some("private-diagnostic".into());
        let text = export_package(&store).unwrap();
        assert!(text.contains("portable-secret"));
        assert!(!text.contains("private-diagnostic"));
        let payload = ConfigPayload::parse(text.as_bytes()).unwrap();
        let report = payload.preview(&StoreSchema::default()).unwrap();
        assert!(!serde_json::to_string(&report)
            .unwrap()
            .contains("portable-secret"));
    }
}

#[cfg(test)]
mod portable_policy_tests {
    use super::*;
    fn source() -> StoreSchema {
        let mut data = StoreSchema::default();
        data.profiles.push(serde_json::from_value(serde_json::json!({"id":"p","name":"Synthetic","kind":"local","content":"proxies: []\nrules: ['MATCH,DIRECT']"})).unwrap());
        data.active_profile = Some("p".into());
        data.process_rules.push(crate::ProcessRule {
            id: "process".into(),
            process: "curl".into(),
            target: "DIRECT".into(),
        });
        data.rule_sets.push(crate::rule_sets::RuleSet {
            id: "set".into(),
            name: "Private list".into(),
            url: "https://example.invalid/list?token=synthetic-secret".into(),
            behavior: crate::rule_sets::Behavior::Domain,
            target: "DIRECT".into(),
            enabled: true,
            interval_secs: 3600,
            payload: vec!["example.invalid".into()],
            updated_at: 1,
            last_error: Some("private-runtime-error".into()),
        });
        data
    }
    #[test]
    fn domain_user_rules_stay_local_and_are_never_exported() {
        // #605 的域名/IP 用户规则不在 v2 承诺内：导出不携带，导入不覆盖。
        let local_rule = crate::user_rules::UserRule {
            id: "local-user-rule".into(),
            kind: crate::user_rules::Kind::Domain,
            value: "local-only.example".into(),
            target: "DIRECT".into(),
            enabled: true,
        };
        let mut target = source();
        target.user_rules = vec![local_rule.clone()];
        let text = export_package(&target).unwrap();
        assert!(!text.contains("local-only.example"));

        let mut destination = StoreSchema {
            user_rules: vec![local_rule],
            ..StoreSchema::default()
        };
        ConfigPayload::parse(export_package(&source()).unwrap().as_bytes())
            .unwrap()
            .merge_into(&mut destination)
            .unwrap();
        assert_eq!(destination.user_rules.len(), 1);
        assert_eq!(destination.user_rules[0].value, "local-only.example");
    }

    #[test]
    fn v2_roundtrip_merges_replaces_without_machine_state_or_downloads() {
        let mut source = source();
        source.engine.tun_enable = true;
        source
            .scenes
            .rules
            .insert("secret-ssid".into(), "proxy".into());
        let text = export_package(&source).unwrap();
        assert!(text.contains("\"package_version\": 2"));
        assert!(text.contains("synthetic-secret"));
        assert!(!text.contains("secret-ssid"));
        assert!(!text.contains("tun_enable"));
        assert!(!text.contains("private-runtime-error"));
        let mut target = StoreSchema::default();
        target.engine.mixed_port = 9999;
        let payload = ConfigPayload::parse(text.as_bytes()).unwrap();
        let report = payload.preview(&target).unwrap();
        assert_eq!((report.added_process_rules, report.added_rule_sets), (1, 1));
        assert!(!serde_json::to_string(&report)
            .unwrap()
            .contains("synthetic-secret"));
        payload.merge_into(&mut target).unwrap();
        assert_eq!(target.process_rules, source.process_rules);
        assert_eq!(target.rule_sets[0].payload, source.rule_sets[0].payload);
        assert_eq!(target.engine.mixed_port, 9999);
        assert!(!target.engine.tun_enable);
        assert!(target.scenes.rules.is_empty());
        let payload = ConfigPayload::parse(text.as_bytes()).unwrap();
        let report = payload.merge_into(&mut target).unwrap();
        assert_eq!(
            (report.replaced_process_rules, report.replaced_rule_sets),
            (1, 1)
        );
        assert_eq!(target.process_rules.len(), 1);
        assert_eq!(target.rule_sets.len(), 1);
        let dir = tempfile::tempdir().unwrap();
        let mut store = crate::Store::open(dir.path()).unwrap();
        *store.data_mut() = target;
        store.save().unwrap();
        let reopened = crate::Store::open(dir.path()).unwrap();
        assert_eq!(reopened.data().process_rules, source.process_rules);
    }
    #[test]
    fn legacy_v1_preserves_destination_policies_and_future_versions_fail() {
        let mut value: serde_json::Value =
            serde_json::from_str(include_str!("../../../docs/config-package/example.json"))
                .unwrap();
        assert_eq!(value["package_version"], 1);
        let sample = ConfigPayload::parse(include_bytes!(
            "../../../docs/config-package/example-v2.json"
        ))
        .unwrap();
        let mut clean = StoreSchema::default();
        sample.merge_into(&mut clean).unwrap();
        assert_eq!(clean.process_rules.len(), 1);
        assert_eq!(clean.rule_sets.len(), 1);
        assert!(!clean.rule_sets[0].enabled);
        let mut target = source();
        ConfigPayload::parse(&serde_json::to_vec(&value).unwrap())
            .unwrap()
            .merge_into(&mut target)
            .unwrap();
        assert_eq!(target.process_rules.len(), 1);
        assert_eq!(target.rule_sets.len(), 1);
        value["package_version"] = serde_json::json!(3);
        assert!(matches!(
            ConfigPayload::parse(&serde_json::to_vec(&value).unwrap()),
            Err(PackageError::Version)
        ));
    }
    #[test]
    fn duplicate_policy_names_reject_before_any_mutation() {
        let source = source();
        let mut target = source.clone();
        target.process_rules[0].id = "different-id".into();
        let payload = ConfigPayload::parse(export_package(&source).unwrap().as_bytes()).unwrap();
        assert_eq!(payload.preview(&target).unwrap().policy_conflicts, 1);
        let before = serde_json::to_vec(&target).unwrap();
        assert!(payload.merge_into(&mut target).is_err());
        assert_eq!(serde_json::to_vec(&target).unwrap(), before);
    }
    #[test]
    fn invalid_cache_and_policy_rows_are_rejected_with_fixed_categories() {
        let original: serde_json::Value =
            serde_json::from_str(&export_package(&source()).unwrap()).unwrap();
        for (pointer, bad) in [
            (
                "/payload/process_rules/0/process",
                serde_json::json!("bad,secret"),
            ),
            (
                "/payload/rule_sets/0/payload",
                serde_json::json!(["bad,secret"]),
            ),
            (
                "/payload/rule_sets/0/url",
                serde_json::json!("file:///private/secret"),
            ),
        ] {
            let mut value = original.clone();
            *value.pointer_mut(pointer).unwrap() = bad;
            assert!(matches!(
                ConfigPayload::parse(&serde_json::to_vec(&value).unwrap()),
                Err(PackageError::Policies)
            ));
        }
    }
    #[test]
    fn missing_policy_is_not_silently_rewritten_to_direct() {
        let mut source = source();
        source.process_rules[0].target = "missing-policy".into();
        let payload = ConfigPayload::parse(export_package(&source).unwrap().as_bytes()).unwrap();
        assert_eq!(
            payload
                .preview(&StoreSchema::default())
                .unwrap()
                .unverified_targets,
            1
        );
        assert!(payload.validate_targets("proxy-groups: []").is_err());
        assert!(payload
            .validate_targets("proxy-groups: [{name: missing-policy}]")
            .is_ok());
        assert!(payload
            .validate_targets("proxies: [{name: missing-policy}]")
            .is_err());
    }
    #[test]
    #[ignore = "requires explicit CB_PACKAGE_MIHOMO; isolated loopback only"]
    fn native_migrated_cache_blocks_and_disabling_restores_direct() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::process::{Command, Stdio};
        let binary = std::env::var("CB_PACKAGE_MIHOMO").expect("CB_PACKAGE_MIHOMO");
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let target = server.local_addr().unwrap().port();
        let worker = std::thread::spawn(move || {
            server.set_nonblocking(true).unwrap();
            let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while std::time::Instant::now() < until {
                match server.accept() {
                    Ok((mut s, _)) => {
                        let mut b = [0; 4096];
                        let _ = s.read(&mut b);
                        let _=s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nmigrated");
                        return;
                    }
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
                }
            }
        });
        let mut source = source();
        source.process_rules[0].process = "synthetic-nonexistent-process".into();
        source.rule_sets[0].behavior = crate::rule_sets::Behavior::Ipcidr;
        source.rule_sets[0].target = "REJECT".into();
        source.rule_sets[0].payload = vec!["127.0.0.1/32".into()];
        let dir = tempfile::tempdir().unwrap();
        let mut store = crate::Store::open(dir.path()).unwrap();
        ConfigPayload::parse(export_package(&source).unwrap().as_bytes())
            .unwrap()
            .merge_into(store.data_mut())
            .unwrap();
        store.save().unwrap();
        drop(store);
        let mut store = crate::Store::open(dir.path()).unwrap();
        for enabled in [true, false] {
            store.data_mut().rule_sets[0].enabled = enabled;
            let port = TcpListener::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap()
                .port();
            let rendered = crate::render_config(store.data()).unwrap();
            assert!(rendered
                .config
                .contains("PROCESS-NAME,synthetic-nonexistent-process,DIRECT"));
            let mut doc: serde_json::Value = serde_yaml::from_str(&rendered.config).unwrap();
            doc["mixed-port"] = serde_json::json!(port);
            doc["bind-address"] = serde_json::json!("127.0.0.1");
            doc["allow-lan"] = serde_json::json!(false);
            let path = dir.path().join("native.json");
            std::fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
            struct Child(std::process::Child);
            impl Drop for Child {
                fn drop(&mut self) {
                    let _ = self.0.kill();
                    let _ = self.0.wait();
                }
            }
            let _core = Child(
                Command::new(&binary)
                    .args([
                        "-f",
                        path.to_str().unwrap(),
                        "-d",
                        dir.path().to_str().unwrap(),
                    ])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap(),
            );
            for _ in 0..100 {
                if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
            let output = Command::new("curl")
                .args([
                    "-fsS",
                    "--max-time",
                    "5",
                    "--noproxy",
                    "",
                    "--proxy",
                    &format!("http://127.0.0.1:{port}"),
                    &format!("http://127.0.0.1:{target}/"),
                ])
                .output()
                .unwrap();
            if enabled {
                assert!(!output.status.success());
            } else {
                assert!(output.status.success());
                assert_eq!(output.stdout, b"migrated");
            }
        }
        worker.join().unwrap();
    }

    #[test]
    fn transactional_save_failure_keeps_policies_and_disk() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = crate::Store::open(dir.path()).unwrap();
        store.save().unwrap();
        let before = std::fs::read(dir.path().join("store.json")).unwrap();
        let state = serde_json::to_vec(store.data()).unwrap();
        let payload = ConfigPayload::parse(export_package(&source()).unwrap().as_bytes()).unwrap();
        let _fault = crate::secure_write::inject_path_at(&dir.path().join("store.json"), 2);
        assert!(store
            .transaction(|data| payload
                .merge_into(data)
                .map(|_| ())
                .map_err(|e| e.to_string()))
            .is_err());
        assert_eq!(serde_json::to_vec(store.data()).unwrap(), state);
        assert_eq!(
            std::fs::read(dir.path().join("store.json")).unwrap(),
            before
        );
    }
}
