//! 核心实体与 schema 版本化。
//!
//! 升级不丢配置的两条底线：
//! 1. 任何 schema 变更必须带 `migrate` 分支与迁移测试；
//! 2. 旧 schema 写入前自动快照（见 `store::Store::save`）。

use crate::emoji::default_flag_emoji;
use serde::{Deserialize, Serialize};

/// 当前存储 schema 版本。每次不兼容变更 +1，并在 `migrate` 中补一条迁移。
pub const SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileKind {
    /// 远程订阅。
    Remote,
    /// 本地文件/手写配置。
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub kind: ProfileKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// 原始订阅内容（Clash YAML / mihomo YAML）。
    #[serde(default)]
    pub content: String,
    /// 自动更新间隔（分钟），0 = 关闭。
    #[serde(default)]
    pub update_interval_min: u32,
    /// 挂载的覆写链，按序应用。
    #[serde(default)]
    pub override_ids: Vec<String>,
    #[serde(default)]
    pub last_updated: Option<u64>,
    /// 订阅流量信息（来自 `subscription-userinfo` 响应头），本地配置为空。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traffic: Option<TrafficInfo>,
    /// 最近一次（含自动）订阅更新结果；旧数据无此字段，缺省 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_update: Option<UpdateOutcome>,
}

/// 一次订阅更新的结果（成功或失败）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateOutcome {
    pub ok: bool,
    /// 失败原因等补充信息。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Unix 秒。
    pub at: u64,
}

/// 机场订阅的流量/到期信息。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TrafficInfo {
    pub upload: u64,
    pub download: u64,
    pub total: u64,
    /// Unix 秒；0 表示未知。
    #[serde(default)]
    pub expire: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OverrideKind {
    /// YAML 顶层合并补丁。
    Merge,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverrideDef {
    pub id: String,
    pub name: String,
    pub kind: OverrideKind,
    pub enabled: bool,
    /// merge 补丁内容（YAML）。
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    /// mihomo（原 Clash.Meta），macOS 默认。
    Mihomo,
    /// sing-box，macOS 备选 / iOS 唯一。
    SingBox,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineConfig {
    #[serde(default = "default_engine")]
    pub engine: Engine,
    /// stable / alpha 通道（M1 引入双通道下载）。
    #[serde(default)]
    pub channel: String,
    #[serde(default = "default_mixed_port")]
    pub mixed_port: u16,
    #[serde(default)]
    pub allow_lan: bool,
    /// 节点旗帜补全（按地区关键词给节点名加国旗前缀）。
    #[serde(default = "default_flag_emoji")]
    pub flag_emoji: bool,
}

fn default_engine() -> Engine {
    Engine::Mihomo
}

fn default_mixed_port() -> u16 {
    7897
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            engine: Engine::Mihomo,
            channel: "stable".into(),
            mixed_port: default_mixed_port(),
            allow_lan: false,
            flag_emoji: default_flag_emoji(),
        }
    }
}

/// 持久化的顶层存储。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreSchema {
    /// 缺省视为 v0（早期手编文件），由 `migrate` 补齐。
    #[serde(default)]
    pub schema_version: u32,
    pub profiles: Vec<Profile>,
    pub overrides: Vec<OverrideDef>,
    #[serde(default)]
    pub engine: EngineConfig,
    /// 当前激活的 profile id。
    #[serde(default)]
    pub active_profile: Option<String>,
    /// v2：界面设置（主题/语言）。
    #[serde(default)]
    pub ui: UiSettings,
}

/// 界面设置（v2 加入）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiSettings {
    /// system / light / dark
    #[serde(default = "default_theme")]
    pub theme: String,
    /// zh / en
    #[serde(default = "default_lang")]
    pub lang: String,
}

fn default_theme() -> String {
    "system".into()
}

fn default_lang() -> String {
    "zh".into()
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            lang: default_lang(),
        }
    }
}

impl Default for StoreSchema {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            profiles: Vec::new(),
            overrides: Vec::new(),
            engine: EngineConfig::default(),
            active_profile: None,
            ui: UiSettings::default(),
        }
    }
}

impl StoreSchema {
    /// 老版本数据迁移到当前版本。写入前由 `Store::save` 调用。
    pub fn migrate(&mut self) {
        // v0 → v1：v0 是无 schema_version 字段的早期手编文件。
        if self.schema_version < 1 {
            // 首个版本：补齐 engine 默认值即可（serde default 已兜底）。
            self.schema_version = 1;
        }
        // v1 → v2：新增 ui 设置（serde default 已兜底，仅需推进版本号）。
        if self.schema_version < 2 {
            self.schema_version = 2;
        }
    }

    pub fn profile(&self, id: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    /// 新增 Profile；id 冲突报错。
    pub fn add_profile(&mut self, profile: Profile) -> Result<(), String> {
        if self.profile(&profile.id).is_some() {
            return Err(format!("profile {} already exists", profile.id));
        }
        if self.active_profile.is_none() {
            self.active_profile = Some(profile.id.clone());
        }
        self.profiles.push(profile);
        Ok(())
    }

    /// 设为当前；不存在的 id 报错。
    pub fn set_active_profile(&mut self, id: &str) -> Result<(), String> {
        if self.profile(id).is_none() {
            return Err(format!("profile {id} not found"));
        }
        self.active_profile = Some(id.to_string());
        Ok(())
    }

    /// 删除 Profile 并清理引用（当前激活、其他档案的覆写链不受影响）。
    pub fn remove_profile(&mut self, id: &str) -> Result<(), String> {
        let before = self.profiles.len();
        self.profiles.retain(|p| p.id != id);
        if self.profiles.len() == before {
            return Err(format!("profile {id} not found"));
        }
        if self.active_profile.as_deref() == Some(id) {
            self.active_profile = self.profiles.first().map(|p| p.id.clone());
        }
        Ok(())
    }

    /// 记录档案更新结果；不存在的 id 报错。
    pub fn set_profile_outcome(&mut self, id: &str, outcome: UpdateOutcome) -> Result<(), String> {
        let p = self
            .profiles
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| format!("profile {id} not found"))?;
        p.last_update = Some(outcome);
        Ok(())
    }

    /// 更新档案内容（订阅刷新）。不存在的 id 报错。
    pub fn update_profile_content(
        &mut self,
        id: &str,
        content: String,
        last_updated: u64,
        traffic: Option<TrafficInfo>,
    ) -> Result<(), String> {
        let p = self
            .profiles
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| format!("profile {id} not found"))?;
        p.content = content;
        p.last_updated = Some(last_updated);
        p.traffic = traffic;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(id: &str) -> Profile {
        Profile {
            id: id.into(),
            name: format!("name-{id}"),
            kind: ProfileKind::Remote,
            url: None,
            content: String::new(),
            update_interval_min: 0,
            override_ids: Vec::new(),
            last_updated: None,
            traffic: None,
            last_update: None,
        }
    }

    #[test]
    fn v0_store_migrates_to_current() {
        let mut data: StoreSchema =
            serde_json::from_str(r#"{"profiles":[],"overrides":[]}"#).unwrap();
        assert_eq!(data.schema_version, 0);
        data.migrate();
        assert_eq!(data.schema_version, SCHEMA_VERSION);
        assert_eq!(data.engine.mixed_port, 7897);
        assert_eq!(data.ui.theme, "system");
    }

    #[test]
    fn profile_without_last_update_field_loads() {
        let raw = r#"{"id":"a","name":"n","kind":"remote","content":""}"#;
        let p: Profile = serde_json::from_str(raw).unwrap();
        assert!(p.last_update.is_none());
    }

    #[test]
    fn v1_store_migrates_to_v2() {
        let mut data: StoreSchema = serde_json::from_str(
            r#"{"schema_version":1,"profiles":[],"overrides":[],"engine":{},"active_profile":null}"#,
        )
        .unwrap();
        data.migrate();
        assert_eq!(data.schema_version, 2);
        assert_eq!(data.ui, UiSettings::default());
    }

    #[test]
    fn future_schema_is_rejected_upstream() {
        let data: StoreSchema =
            serde_json::from_str(r#"{"schema_version":99,"profiles":[],"overrides":[]}"#).unwrap();
        // Store::load 负责拒绝；这里只断言反序列化本身不丢信息。
        assert_eq!(data.schema_version, 99);
    }

    #[test]
    fn add_first_profile_makes_it_active() {
        let mut s = StoreSchema::default();
        s.add_profile(profile("a")).unwrap();
        assert_eq!(s.active_profile.as_deref(), Some("a"));
        s.add_profile(profile("b")).unwrap();
        assert_eq!(s.active_profile.as_deref(), Some("a"));
        assert!(s.add_profile(profile("a")).is_err());
    }

    #[test]
    fn remove_active_profile_falls_back_to_first() {
        let mut s = StoreSchema::default();
        s.add_profile(profile("a")).unwrap();
        s.add_profile(profile("b")).unwrap();
        s.set_active_profile("b").unwrap();
        s.remove_profile("b").unwrap();
        assert_eq!(s.active_profile.as_deref(), Some("a"));
        assert!(s.remove_profile("zz").is_err());
    }

    #[test]
    fn update_content_sets_timestamp_and_traffic() {
        let mut s = StoreSchema::default();
        s.add_profile(profile("a")).unwrap();
        s.update_profile_content(
            "a",
            "proxies: []".into(),
            42,
            Some(TrafficInfo {
                upload: 1,
                download: 2,
                total: 10,
                expire: 0,
            }),
        )
        .unwrap();
        let p = s.profile("a").unwrap();
        assert_eq!(p.last_updated, Some(42));
        assert_eq!(p.traffic.unwrap().total, 10);
    }
}
