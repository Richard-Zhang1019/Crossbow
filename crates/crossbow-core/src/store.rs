//! 持久化存储：JSON 落盘 + 写前快照（升级不丢配置的兜底）。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::model::{StoreSchema, SCHEMA_VERSION};
use crate::{Error, Result};

/// 快照保留数量。
const MAX_SNAPSHOTS: usize = 5;

pub struct Store {
    path: PathBuf,
    data: StoreSchema,
}

impl Store {
    /// 从 `dir/store.json` 加载；不存在则创建默认存储。
    pub fn open(dir: &Path) -> Result<Self> {
        fs::create_dir_all(dir)?;
        let path = dir.join("store.json");
        let data = if path.exists() {
            let raw = fs::read_to_string(&path)?;
            Self::load_str(&raw)?
        } else {
            StoreSchema::default()
        };
        Ok(Self { path, data })
    }

    /// 解析并迁移存储内容；未来版本直接拒绝（防止旧程序写坏新数据）。
    pub fn load_str(raw: &str) -> Result<StoreSchema> {
        let mut data: StoreSchema = serde_json::from_str(raw)?;
        if data.schema_version > SCHEMA_VERSION {
            return Err(Error::SchemaTooNew {
                found: data.schema_version,
                supported: SCHEMA_VERSION,
            });
        }
        data.migrate();
        Ok(data)
    }

    pub fn data(&self) -> &StoreSchema {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut StoreSchema {
        &mut self.data
    }

    /// 保存：先把现有文件轮转为快照，再原子写入（tmp + rename）。
    pub fn save(&mut self) -> Result<()> {
        self.data.schema_version = SCHEMA_VERSION;
        if self.path.exists() {
            self.rotate_snapshot()?;
        }
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(&self.data)?)?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    fn rotate_snapshot(&self) -> Result<()> {
        let snap_dir = self.path.parent().unwrap().join("snapshots");
        fs::create_dir_all(&snap_dir)?;
        // 纳秒精度：同一秒内连续保存也不互相覆盖。
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        fs::copy(&self.path, snap_dir.join(format!("store-{ts}.json")))?;
        let mut snaps: Vec<PathBuf> = fs::read_dir(&snap_dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        snaps.sort();
        while snaps.len() > MAX_SNAPSHOTS {
            let oldest = snaps.remove(0);
            let _ = fs::remove_file(oldest);
        }
        Ok(())
    }

    /// 列出可用快照（设置页「回滚配置」用）。
    pub fn snapshots(&self) -> Result<Vec<PathBuf>> {
        let snap_dir = self.path.parent().unwrap().join("snapshots");
        if !snap_dir.exists() {
            return Ok(Vec::new());
        }
        let mut snaps: Vec<PathBuf> = fs::read_dir(&snap_dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        snaps.sort();
        Ok(snaps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = tmp_dir();
        let mut store = Store::open(dir.path()).unwrap();
        store.data_mut().active_profile = Some("p1".into());
        store.save().unwrap();

        let reloaded = Store::open(dir.path()).unwrap();
        assert_eq!(reloaded.data().active_profile.as_deref(), Some("p1"));
    }

    #[test]
    fn save_keeps_snapshots_and_caps_count() {
        let dir = tmp_dir();
        let mut store = Store::open(dir.path()).unwrap();
        for i in 0..8 {
            store.data_mut().active_profile = Some(format!("p{i}"));
            store.save().unwrap();
        }
        let snaps = store.snapshots().unwrap();
        assert_eq!(snaps.len(), MAX_SNAPSHOTS);
        // 快照内容是上一次保存前的状态。
        let raw = fs::read_to_string(snaps.last().unwrap()).unwrap();
        assert!(raw.contains("\"p6\""));
    }

    #[test]
    fn future_schema_rejected() {
        let err = Store::load_str(r#"{"schema_version":99,"profiles":[],"overrides":[]}"#);
        assert!(matches!(err, Err(Error::SchemaTooNew { .. })));
    }
}
