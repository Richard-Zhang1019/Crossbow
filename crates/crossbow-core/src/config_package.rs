//! Versioned local migration of profiles, overrides and the active selection.
//! Package contents can contain credentials. Errors and previews use fixed
//! categories and counts only.

use crate::model::{OverrideDef, OverrideKind, Profile, StoreSchema, SCHEMA_VERSION};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const PACKAGE_VERSION: u32 = 1;
pub const MAX_PACKAGE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct ConfigPackage {
    package_version: u32,
    #[serde(default)]
    created_at: u64,
    schema_version: u32,
    payload: ConfigPayload,
}

#[derive(Serialize, Deserialize)]
pub struct ConfigPayload {
    profiles: Vec<Profile>,
    overrides: Vec<OverrideDef>,
    #[serde(default)]
    active_profile: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ConfigImportReport {
    pub added_profiles: usize,
    pub replaced_profiles: usize,
    pub added_overrides: usize,
    pub replaced_overrides: usize,
    pub selects_profile: bool,
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
}

pub fn export_package(data: &StoreSchema) -> Result<String, PackageError> {
    let mut payload = ConfigPayload {
        profiles: data.profiles.clone(),
        overrides: data.overrides.clone(),
        active_profile: data.active_profile.clone(),
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

impl ConfigPayload {
    pub fn parse(text: &[u8]) -> Result<Self, PackageError> {
        if text.len() > MAX_PACKAGE_BYTES {
            return Err(PackageError::Size);
        }
        let package: ConfigPackage =
            serde_json::from_slice(text).map_err(|_| PackageError::Format)?;
        if package.package_version != PACKAGE_VERSION {
            return Err(PackageError::Version);
        }
        if package.schema_version > SCHEMA_VERSION {
            return Err(PackageError::Schema);
        }
        // Older supported schemas share these portable fields. Serde fills in
        // optional fields before we validate references and drop diagnostics.
        let mut payload = package.payload;
        payload.clear_diagnostics();
        payload.validate()?;
        Ok(payload)
    }

    fn clear_diagnostics(&mut self) {
        for profile in &mut self.profiles {
            profile.last_update = None;
        }
        for override_def in &mut self.overrides {
            override_def.last_error = None;
        }
    }

    fn validate(&self) -> Result<(), PackageError> {
        if self.profiles.is_empty() && self.overrides.is_empty() {
            return Err(PackageError::Empty);
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
        Ok(ConfigImportReport {
            added_profiles: self.profiles.len().saturating_sub(replaced_profiles),
            replaced_profiles,
            added_overrides: self.overrides.len().saturating_sub(replaced_overrides),
            replaced_overrides,
            selects_profile: self.active_profile.is_some(),
        })
    }

    pub fn changes_active_profile(&self, current: &StoreSchema) -> bool {
        self.active_profile.is_some()
            || current
                .active_profile
                .as_deref()
                .is_some_and(|id| self.profiles.iter().any(|profile| profile.id == id))
    }

    pub fn merge_into(self, current: &mut StoreSchema) -> Result<ConfigImportReport, PackageError> {
        let report = self.preview(current)?;
        let profile_ids: BTreeSet<_> = self.profiles.iter().map(|p| p.id.as_str()).collect();
        let override_ids: BTreeSet<_> = self.overrides.iter().map(|o| o.id.as_str()).collect();
        current
            .profiles
            .retain(|p| !profile_ids.contains(p.id.as_str()));
        current
            .overrides
            .retain(|o| !override_ids.contains(o.id.as_str()));
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
        // 故障开关（write 阶段失败，持注入锁），验证写失败保留原 store。
        let _inject = crate::secure_write::inject_at(2);
        let save_result = store.save();
        assert!(save_result.is_err());

        let reopened = Store::open(dir.path()).unwrap();
        assert_eq!(reopened.data().active_profile.as_deref(), Some("previous"));
        assert_eq!(reopened.data().profiles.len(), 1);
        // 注入点在 rotate 的快照写入（第 2 步）：快照未写出属预期——
        // 核心保证是主文件未被触碰。
        assert_eq!(reopened.snapshots().unwrap().len(), 0);
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
