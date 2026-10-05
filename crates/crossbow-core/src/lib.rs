//! crossbow-core — Crossbow 的共享「大脑」。
//!
//! macOS（Tauri 壳）与 iOS（UniFFI 桥）共用本 crate：
//! Profile/订阅管理、覆写链、配置渲染（→ mihomo YAML）、存储快照与迁移。
//! 平台相关能力（进程管理、系统代理、NE 隧道）不在这里，由各壳实现。

pub mod model;
pub mod override_patch;
pub mod render;
pub mod runtime;
pub mod store;
pub mod subscription;

pub use model::{
    Engine, EngineConfig, OverrideDef, OverrideKind, Profile, ProfileKind, StoreSchema,
    TrafficInfo, UiSettings, UpdateOutcome,
};
pub use render::render_config;
pub use runtime::{apply_runtime, RuntimeConfig};
pub use store::Store;

/// 统一错误类型。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("yaml: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("network: {0}")]
    Network(String),
    #[error("profile {0} not found")]
    ProfileNotFound(String),
    #[error("store schema {found} is newer than supported {supported}")]
    SchemaTooNew { found: u32, supported: u32 },
}

pub type Result<T> = std::result::Result<T, Error>;
