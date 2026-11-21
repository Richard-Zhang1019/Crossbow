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
        crate::secure_write::ensure_private_dir(dir)?;
        let path = dir.join("store.json");
        match crate::secure_write::ensure_private(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let snapshots = dir.join("snapshots");
        match snapshots.symlink_metadata() {
            Ok(_) => {
                crate::secure_write::ensure_private_dir(&snapshots)?;
                for entry in fs::read_dir(&snapshots)? {
                    let entry = entry?;
                    if is_snapshot_name(&entry.file_name()) {
                        crate::secure_write::ensure_private(&entry.path())?;
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
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
        // #595/#596：私有原子写入（0600 + 独占临时文件 + 文件/父目录 sync
        // + 失败清理），替代固定名 tmp 的裸 write+rename。
        crate::secure_write::write_private_atomic(
            &self.path,
            &serde_json::to_vec_pretty(&self.data)?,
        )
        .map_err(Error::Io)
    }

    fn rotate_snapshot(&self) -> Result<()> {
        let snap_dir = self.path.parent().unwrap().join("snapshots");
        // 专用目录 0700（#595）：快照含订阅内容，与主文件同一私有边界。
        crate::secure_write::ensure_private_dir(&snap_dir)?;
        // 纳秒精度：同一秒内连续保存也不互相覆盖。
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        // fs::copy 不设权限（umask 下落 0644），改为读 + 私有安全写入。
        let contents = fs::read(&self.path)?;
        crate::secure_write::write_private_atomic(
            &snap_dir.join(format!("store-{ts}.json")),
            &contents,
        )?;
        let mut snaps = self.snapshots()?;
        while snaps.len() > MAX_SNAPSHOTS {
            let oldest = snaps.remove(0);
            let _ = fs::remove_file(oldest);
        }
        Ok(())
    }

    pub fn snapshot_data(&self, id: &str) -> Result<StoreSchema> {
        let path = self
            .snapshots()?
            .into_iter()
            .find(|p| p.file_name().and_then(|n| n.to_str()) == Some(id))
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "Snapshot not found")
            })?;
        Self::load_str(&fs::read_to_string(path)?)
    }

    /// 列出可用快照（设置页「回滚配置」用）。
    pub fn snapshots(&self) -> Result<Vec<PathBuf>> {
        let snap_dir = self.path.parent().unwrap().join("snapshots");
        if !snap_dir.exists() {
            return Ok(Vec::new());
        }
        crate::secure_write::ensure_private_dir(&snap_dir)?;
        let mut snaps: Vec<PathBuf> = fs::read_dir(&snap_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| is_snapshot_name(&e.file_name()))
            .filter(|e| e.file_type().is_ok_and(|kind| kind.is_file()))
            .map(|e| e.path())
            .collect();
        snaps.sort();
        Ok(snaps)
    }
}

fn is_snapshot_name(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .and_then(|name| name.strip_prefix("store-"))
        .and_then(|name| name.strip_suffix(".json"))
        .is_some_and(|timestamp| {
            !timestamp.is_empty() && timestamp.bytes().all(|b| b.is_ascii_digit())
        })
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

    #[cfg(unix)]
    #[test]
    fn open_migrates_owned_legacy_snapshots_only() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::open(root.path()).unwrap();
        store.save().unwrap();
        store.save().unwrap();
        let snapshot = store.snapshots().unwrap().pop().unwrap();
        let snapshots = root.path().join("snapshots");
        let unrelated = snapshots.join("notes.txt");
        fs::write(&unrelated, b"unrelated").unwrap();
        for path in [&root.path().join("store.json"), &snapshot, &unrelated] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
        }
        fs::set_permissions(&snapshots, fs::Permissions::from_mode(0o755)).unwrap();
        Store::open(root.path()).unwrap();
        assert_eq!(
            snapshot.metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            snapshots.metadata().unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            unrelated.metadata().unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(fs::read(&unrelated).unwrap(), b"unrelated");
        for _ in 0..8 {
            store.save().unwrap();
        }
        assert_eq!(store.snapshots().unwrap().len(), MAX_SNAPSHOTS);
        assert_eq!(fs::read(&unrelated).unwrap(), b"unrelated");
    }

    #[cfg(unix)]
    #[test]
    fn open_rejects_linked_store_and_snapshot_directories() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let root = tempfile::tempdir().unwrap();
        let outside = root.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o755)).unwrap();
        let linked_store = root.path().join("linked-store");
        symlink(&outside, &linked_store).unwrap();
        assert!(Store::open(&linked_store).is_err());
        let local = root.path().join("local");
        fs::create_dir(&local).unwrap();
        symlink(&outside, local.join("snapshots")).unwrap();
        assert!(Store::open(&local).is_err());
        assert_eq!(
            outside.metadata().unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn primary_file_failures_preserve_saved_data_and_completed_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("store.json");
        let mut store = Store::open(root.path()).unwrap();
        store.data_mut().active_profile = Some("previous".into());
        store.save().unwrap();
        let before = fs::read(&path).unwrap();
        store.data_mut().active_profile = Some("candidate".into());
        for stage in 1..=4 {
            let guard = crate::secure_write::inject_path_at(&path, stage);
            assert!(store.save().is_err());
            drop(guard);
            assert_eq!(fs::read(&path).unwrap(), before);
            assert_eq!(store.snapshots().unwrap().len(), stage);
            for snapshot in store.snapshots().unwrap() {
                assert_eq!(fs::read(snapshot).unwrap(), before);
            }
            assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
        }
        store.save().unwrap();
        assert_eq!(
            Store::open(root.path())
                .unwrap()
                .data()
                .active_profile
                .as_deref(),
            Some("candidate")
        );
    }

    #[test]
    fn future_schema_rejected() {
        let err = Store::load_str(r#"{"schema_version":99,"profiles":[],"overrides":[]}"#);
        assert!(matches!(err, Err(Error::SchemaTooNew { .. })));
    }
}
