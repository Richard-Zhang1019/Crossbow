//! #510：带版本号的本地配置包——导出、校验、预览与导入报告。
//!
//! 包内容只覆盖可迁移面：订阅来源、当前配置内容、覆写链与当前激活项。
//! 机器相关选项（引擎/端口/UI/日志/更新设置）与运行时状态（系统代理、
//! TUN、系统授权）不随包迁移——导入方保留自己的这些字段。
//! 敏感边界：profiles 内含订阅 URL 与节点凭据，导出前由前端提示；
//! 报告与错误不包含凭据字段。

use crate::model::{StoreSchema, SCHEMA_VERSION};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// 配置包格式版本。解析时严格等于 1——未来版本由导入方明确拒绝。
pub const PACKAGE_VERSION: u32 = 1;

/// 单文件读取上限：配置包是结构化 JSON，超限多半是误选文件。
pub const MAX_PACKAGE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConfigPackage {
    package_version: u32,
    #[serde(default)]
    created_at: u64,
    schema_version: u32,
    payload: ConfigPayload,
}

/// 可迁移面：订阅 + 覆写链 + 当前激活项。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigPayload {
    pub profiles: Vec<crate::model::Profile>,
    pub overrides: Vec<crate::model::OverrideDef>,
    #[serde(default)]
    pub active_profile: Option<String>,
}

/// 导入预览/结果报告（脱敏：只有计数与 id，不含 URL 与内容）。
#[derive(Debug, Clone, Serialize)]
pub struct ConfigImportReport {
    pub package_version: u32,
    pub added_profiles: u32,
    pub replaced_profiles: u32,
    pub added_overrides: u32,
    pub replaced_overrides: u32,
    pub active_profile: Option<String>,
    /// 不可静默处理的警告（如激活项在包内不存在——将回落无激活）。
    pub warnings: Vec<String>,
}

/// 从当前数据构造配置包 JSON（pretty）。只取可迁移字段。
pub fn export_package(data: &StoreSchema) -> String {
    let package = ConfigPackage {
        package_version: PACKAGE_VERSION,
        created_at: now_secs(),
        schema_version: data.schema_version,
        payload: ConfigPayload {
            profiles: data.profiles.clone(),
            overrides: data.overrides.clone(),
            active_profile: data.active_profile.clone(),
        },
    };
    serde_json::to_string_pretty(&package).unwrap_or_else(|_| "{}".into())
}

/// 解析并校验包文本，产出导入预览报告（不写库）。`current` 为导入方
/// 现有数据，用于计算新增/替换计数。
pub fn preview_package(text: &str, current: &StoreSchema) -> Result<ConfigImportReport, String> {
    if text.len() > MAX_PACKAGE_BYTES {
        return Err(format!(
            "配置包超过读取上限（{} 字节 > {}）",
            text.len(),
            MAX_PACKAGE_BYTES
        ));
    }
    let package: ConfigPackage = serde_json::from_str(text)
        .map_err(|e| format!("配置包无法解析（JSON 损坏或格式不符）：{e}"))?;
    if package.package_version != PACKAGE_VERSION {
        return Err(format!(
            "配置包版本不符：需要 {PACKAGE_VERSION}，实际 {}（可能来自更新或更旧的应用）",
            package.package_version
        ));
    }
    if package.schema_version > SCHEMA_VERSION {
        return Err(format!(
            "配置包 schema v{} 高于当前支持的 v{SCHEMA_VERSION}——请先升级应用",
            package.schema_version
        ));
    }
    validate_payload(&package.payload)?;

    // 冲突/新增计数（按 profile id）
    let existing_ids: BTreeSet<&str> = current.profiles.iter().map(|p| p.id.as_str()).collect();
    let package_ids: BTreeSet<&str> = package
        .payload
        .profiles
        .iter()
        .map(|p| p.id.as_str())
        .collect();
    let replaced_profiles = existing_ids.intersection(&package_ids).count() as u32;
    let added_profiles =
        (package_ids.len() - existing_ids.intersection(&package_ids).count()) as u32;

    let existing_overrides: BTreeSet<&str> =
        current.overrides.iter().map(|o| o.id.as_str()).collect();
    let package_overrides: BTreeSet<&str> = package
        .payload
        .overrides
        .iter()
        .map(|o| o.id.as_str())
        .collect();
    let replaced_overrides = existing_overrides.intersection(&package_overrides).count() as u32;
    let added_overrides = (package_overrides.len()
        - existing_overrides.intersection(&package_overrides).count())
        as u32;

    let mut warnings = Vec::new();
    if let Some(active) = &package.payload.active_profile {
        if !package_ids.contains(active.as_str()) {
            warnings.push(format!(
                "包内激活项 {active} 不在包内订阅列表中，导入后不设置激活"
            ));
        }
    }
    if !current.profiles.is_empty() && replaced_profiles > 0 {
        warnings.push(format!(
            "将替换现有 {replaced_profiles} 个同 id 订阅（导入前会自动生成恢复快照）"
        ));
    }

    Ok(ConfigImportReport {
        package_version: package.package_version,
        added_profiles,
        replaced_profiles,
        added_overrides,
        replaced_overrides,
        active_profile: package.payload.active_profile.clone(),
        warnings,
    })
}

/// 校验包内容：订阅/覆写条目完整性与 id 唯一性。任何一项失败都拒绝写入。
fn validate_payload(payload: &ConfigPayload) -> Result<(), String> {
    if payload.profiles.is_empty() {
        return Err("配置包内没有任何订阅配置".into());
    }
    let mut seen_profile = BTreeSet::new();
    for profile in &payload.profiles {
        if profile.id.trim().is_empty() || profile.name.trim().is_empty() {
            return Err("配置包含未命名或空 id 的订阅条目".into());
        }
        if !seen_profile.insert(profile.id.as_str()) {
            return Err(format!("配置包内订阅 id 重复：{}", profile.id));
        }
    }
    let mut seen_override = BTreeSet::new();
    for override_def in &payload.overrides {
        if override_def.id.trim().is_empty() || override_def.name.trim().is_empty() {
            return Err("配置包含未命名或空 id 的覆写条目".into());
        }
        if !seen_override.insert(override_def.id.as_str()) {
            return Err(format!("配置包内覆写 id 重复：{}", override_def.id));
        }
    }
    Ok(())
}

/// 把预览通过的包内容应用到数据（仅供导入命令的事务回调使用）。
/// 机器相关字段（engine/ui/logs/update/接管状态）不动。
pub fn apply_package(data: &mut StoreSchema, payload: &ConfigPayload) {
    data.profiles = payload.profiles.clone();
    data.overrides = payload.overrides.clone();
    data.active_profile = payload.active_profile.clone();
}

pub fn parse_package(text: &str) -> Result<ConfigPayload, String> {
    if text.len() > MAX_PACKAGE_BYTES {
        return Err(format!(
            "配置包超过读取上限（{} 字节 > {}）",
            text.len(),
            MAX_PACKAGE_BYTES
        ));
    }
    let package: ConfigPackage = serde_json::from_str(text)
        .map_err(|e| format!("配置包无法解析（JSON 损坏或格式不符）：{e}"))?;
    if package.package_version != PACKAGE_VERSION {
        return Err(format!(
            "配置包版本不符：需要 {PACKAGE_VERSION}，实际 {}",
            package.package_version
        ));
    }
    if package.schema_version > SCHEMA_VERSION {
        return Err(format!(
            "配置包 schema v{} 高于当前支持的 v{SCHEMA_VERSION}——请先升级应用",
            package.schema_version
        ));
    }
    validate_payload(&package.payload)?;
    Ok(package.payload)
}

pub(crate) fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Profile, ProfileKind};

    fn sample_profile(id: &str, name: &str) -> Profile {
        Profile {
            id: id.into(),
            name: name.into(),
            kind: ProfileKind::Remote,
            url: None,
            content: "proxies: []".into(),
            update_interval_min: 0,
            override_ids: vec![],
            last_updated: None,
            traffic: None,
            last_update: None,
        }
    }

    fn base_store() -> StoreSchema {
        StoreSchema::default()
    }

    fn package_text(profiles: &[Profile], overrides_json: &str) -> String {
        format!(
            r#"{{"package_version":1,"created_at":1,"schema_version":{SCHEMA_VERSION},
                "payload":{{"profiles":{},"overrides":{overrides_json},"active_profile":null}}}}"#,
            serde_json::to_string(profiles).unwrap()
        )
    }

    #[test]
    fn export_round_trips_through_preview() {
        let mut data = base_store();
        data.profiles.push(sample_profile("p1", "主订阅"));
        data.overrides.push(crate::model::OverrideDef {
            id: "o1".into(),
            name: "覆写A".into(),
            kind: crate::model::OverrideKind::Merge,
            enabled: true,
            content: "rules-prepend: []".into(),
            last_error: None,
        });
        data.active_profile = Some("p1".into());

        let text = export_package(&data);
        let report = preview_package(&text, &base_store()).unwrap();
        assert_eq!(report.added_profiles, 1);
        assert_eq!(report.added_overrides, 1);
        assert_eq!(report.active_profile.as_deref(), Some("p1"));
    }

    #[test]
    fn future_package_version_is_rejected() {
        let text = format!(
            r#"{{"package_version":99,"schema_version":{SCHEMA_VERSION},"payload":{{"profiles":[],"overrides":[]}}}}"#
        );
        let err = preview_package(&text, &base_store()).unwrap_err();
        assert!(err.contains("版本不符"), "{err}");
    }

    #[test]
    fn future_schema_is_rejected() {
        let text = format!(
            r#"{{"package_version":1,"schema_version":{},"payload":{{"profiles":[],"overrides":[]}}}}"#,
            SCHEMA_VERSION + 1
        );
        let err = preview_package(&text, &base_store()).unwrap_err();
        assert!(err.contains("高于当前支持"), "{err}");
    }

    #[test]
    fn corrupted_json_is_rejected() {
        let err = preview_package("{ not json", &base_store()).unwrap_err();
        assert!(err.contains("无法解析"), "{err}");
    }

    #[test]
    fn non_string_rule_like_garbage_profiles_rejected() {
        let text = format!(
            r#"{{"package_version":1,"schema_version":{SCHEMA_VERSION},"payload":{{"profiles":[],"overrides":[]}}}}"#
        );
        let err = preview_package(&text, &base_store()).unwrap_err();
        assert!(err.contains("没有任何订阅"), "{err}");
    }

    #[test]
    fn duplicate_profile_ids_rejected() {
        let profiles = vec![sample_profile("p1", "A"), sample_profile("p1", "B")];
        let text = package_text(&profiles, "[]");
        let err = preview_package(&text, &base_store()).unwrap_err();
        assert!(err.contains("id 重复"), "{err}");
    }

    #[test]
    fn replace_counts_by_id() {
        let mut current = base_store();
        current.profiles.push(sample_profile("p1", "旧A"));
        let text = package_text(
            &[sample_profile("p1", "新A"), sample_profile("p2", "B")],
            "[]",
        );
        let report = preview_package(&text, &current).unwrap();
        assert_eq!(report.replaced_profiles, 1);
        assert_eq!(report.added_profiles, 1);
        assert!(report.warnings.iter().any(|w| w.contains("替换现有 1 个")));
    }

    #[test]
    fn active_profile_missing_from_package_warns() {
        let text = format!(
            r#"{{"package_version":1,"schema_version":{SCHEMA_VERSION},"payload":{{
            "profiles":[{{"id":"p1","name":"A","kind":"remote","url":null}}],
            "overrides":[],"active_profile":"ghost"}}}}"#
        );
        let report = preview_package(&text, &base_store()).unwrap();
        assert!(report.warnings.iter().any(|w| w.contains("ghost")));
    }

    #[test]
    fn apply_package_leaves_machine_fields_untouched() {
        let mut data = base_store();
        let original_port = data.engine.mixed_port;
        apply_package(
            &mut data,
            &ConfigPayload {
                profiles: vec![sample_profile("p1", "A")],
                overrides: vec![],
                active_profile: Some("p1".into()),
            },
        );
        assert_eq!(data.engine.mixed_port, original_port);
        assert_eq!(data.profiles.len(), 1);
        assert_eq!(data.active_profile.as_deref(), Some("p1"));
    }
}
