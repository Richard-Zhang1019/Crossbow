//! 持久化存储：JSON 落盘 + 写前快照（升级不丢配置的兜底）。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::model::{StoreSchema, SCHEMA_VERSION};
use crate::{Error, Result};

/// 快照保留数量。
const MAX_SNAPSHOTS: usize = 5;

#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    data: StoreSchema,
    /// #594：启动恢复报告（主文件损坏时经快照/默认恢复的非空记录）。
    /// 桌面层据此向用户反馈恢复来源与原因；None = 正常加载。
    pub recovery: Option<RecoveryReport>,
}

/// #594 启动恢复报告：脱敏（只含文件名/来源/原因短语，不含配置内容）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RecoveryReport {
    /// 恢复来源：`"snapshot:<文件名>"` 或 `"default"`。
    pub source: String,
    /// 损坏主文件的隔离备份文件名（含扩展名），原件可追溯；None = 无隔离。
    pub quarantined_as: Option<String>,
    /// 原因短语（损坏/全坏/无快照），供通知展示。
    pub reason: String,
}

impl Store {
    /// 从 `dir/store.json` 加载；不存在则创建默认存储。
    ///
    /// #594：主文件损坏（解析失败）时进入恢复路径——隔离损坏原件、
    /// 按时间倒序校验快照、恢复最近有效版本或回落默认；恢复全程使用
    /// 私有耐久写入（#595/#596），不直接清空成默认库。权限/IO 类错误
    /// （文件未坏只是读不了）不做恢复，错误向上传播以明确暴露。
    /// 未来 schema 同样向上传播——旧版本不得自动降级覆盖新数据。
    pub fn open(dir: &Path) -> Result<Self> {
        crate::secure_write::ensure_private_dir(dir)?;
        let path = dir.join("store.json");
        let present = match crate::secure_write::ensure_private(&path) {
            Ok(()) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
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
        if present {
            match Self::load_bytes(&fs::read(&path)?) {
                Ok(data) => {
                    return Ok(Self {
                        path,
                        data,
                        recovery: None,
                    })
                }
                Err(error @ Error::SchemaTooNew { .. }) => return Err(error),
                Err(_) => {}
            }
        }

        let mut recovered = None;
        for snapshot in Self::snapshots_valid_sorted(dir)? {
            let raw = fs::read(&snapshot)?;
            match Self::load_bytes(&raw) {
                Ok(data) => {
                    recovered = Some((
                        data,
                        format!(
                            "snapshot:{}",
                            snapshot.file_name().unwrap().to_string_lossy()
                        ),
                    ));
                    break;
                }
                Err(error @ Error::SchemaTooNew { .. }) => return Err(error),
                Err(_) => {}
            }
        }
        // A previous attempt may have quarantined the primary and then failed
        // to write its replacement. Missing primary is not necessarily first run.
        let previous_quarantine = fs::read_dir(dir)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<std::io::Result<Vec<_>>>()?
            .into_iter()
            .filter_map(|name| name.into_string().ok())
            .filter(|name| name.starts_with("store.json.corrupt-"))
            .max();
        if !present && recovered.is_none() && previous_quarantine.is_none() {
            return Ok(Self {
                path,
                data: StoreSchema::default(),
                recovery: None,
            });
        }
        let quarantined = if present {
            Self::quarantine_corrupt(&path)?
        } else {
            previous_quarantine
        };
        let (data, source, reason) = match recovered {
            Some((data, source)) => (data, source, "主文件不可用，已从快照恢复"),
            None => (
                StoreSchema::default(),
                "default".to_string(),
                "主文件不可用，无可用快照，已从默认配置启动",
            ),
        };
        crate::secure_write::write_private_atomic(&path, &serde_json::to_vec_pretty(&data)?)?;
        Ok(Self {
            path,
            data,
            recovery: Some(RecoveryReport {
                source,
                quarantined_as: quarantined,
                reason: reason.to_string(),
            }),
        })
    }

    /// 隔离损坏主文件：`store.json` → `store.json.corrupt-<纳秒>`（同目录
    /// 原子 rename，不删除；返回备份文件名）。已存在同名备份时追加序号。
    fn quarantine_corrupt(path: &Path) -> Result<Option<String>> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let backup = path.with_file_name(format!("store.json.corrupt-{nanos}"));
        fs::rename(path, &backup).map_err(Error::Io)?;
        let name = backup
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Some(name))
    }

    /// 数据目录下的快照列表，文件名倒序（纳秒时间戳命名 = 时间倒序）。
    fn snapshots_valid_sorted(dir: &Path) -> Result<Vec<PathBuf>> {
        let snap_dir = dir.join("snapshots");
        let mut snaps = Vec::new();
        match fs::read_dir(&snap_dir) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry?;
                    if is_snapshot_name(&entry.file_name()) {
                        snaps.push(entry.path());
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        snaps.sort();
        snaps.reverse();
        Ok(snaps)
    }

    /// 解析并迁移存储内容；未来版本直接拒绝（防止旧程序写坏新数据）。
    pub fn load_str(raw: &str) -> Result<StoreSchema> {
        Self::load_bytes(raw.as_bytes())
    }

    fn load_bytes(raw: &[u8]) -> Result<StoreSchema> {
        #[derive(serde::Deserialize)]
        struct Version {
            #[serde(default)]
            schema_version: u32,
        }
        // Inspect the version before interpreting fields that may have changed
        // type or acquired new enum variants in a newer application.
        let version: Version = serde_json::from_slice(raw)?;
        if version.schema_version > SCHEMA_VERSION {
            return Err(Error::SchemaTooNew {
                found: version.schema_version,
                supported: SCHEMA_VERSION,
            });
        }
        let mut data: StoreSchema = serde_json::from_slice(raw)?;
        data.migrate();
        Ok(data)
    }

    pub fn data(&self) -> &StoreSchema {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut StoreSchema {
        &mut self.data
    }

    /// Apply a non-runtime change under the caller's existing store lock.
    /// Candidate state is published only after persistence. If replacement
    /// happened before an error, attempt restoration and read the actual disk
    /// state back, so an error never leaves a fictitious in-memory rollback.
    pub fn transaction<T>(
        &mut self,
        edit: impl FnOnce(&mut StoreSchema) -> std::result::Result<T, String>,
    ) -> std::result::Result<T, String> {
        let mut candidate = Self {
            path: self.path.clone(),
            data: self.data.clone(),
            recovery: None,
        };
        let result = edit(&mut candidate.data)?;
        if let Err(error) = candidate.save() {
            let expected =
                serde_json::to_vec_pretty(&candidate.data).map_err(|error| error.to_string())?;
            let disk = fs::read(&self.path);
            let mut detail = format!("保存失败：{error}");
            if disk.as_deref().ok() == Some(expected.as_slice()) {
                let previous =
                    serde_json::to_vec_pretty(&self.data).map_err(|error| error.to_string())?;
                if let Err(restore) =
                    crate::secure_write::write_private_atomic(&self.path, &previous)
                {
                    detail.push_str(&format!("；恢复写入未能确认完成：{restore}"));
                }
            }
            match fs::read(&self.path) {
                Ok(raw) => match Self::load_bytes(&raw) {
                    Ok(actual) => self.data = actual,
                    Err(_) => detail.push_str("；磁盘配置无法解析，请重新加载后确认状态"),
                },
                Err(read) if read.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => detail.push_str("；无法核实磁盘状态，请重新加载后确认"),
            }
            return Err(detail);
        }
        self.data = candidate.data;
        Ok(result)
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

    /// #594 五场景：主文件损坏+有效快照 / 最新快照也坏+更早有效 / 全坏 /
    /// 无快照 / 权限与 IO 类错误。脱敏断言（只查来源与隔离文件名）。
    #[cfg(test)]
    mod recovery {
        use super::*;

        fn seed_snapshots(dir: &Path, contents: &[(&str, String)]) {
            let snap_dir = dir.join("snapshots");
            fs::create_dir_all(&snap_dir).unwrap();
            for (name, content) in contents {
                fs::write(snap_dir.join(name), content).unwrap();
            }
        }

        fn valid_schema_json(active: &str) -> String {
            format!(
                r#"{{"schema_version":2,"profiles":[{{"id":"{a}","name":"{a}","kind":"local"}}],"overrides":[],"active_profile":"{a}"}}"#,
                a = active
            )
        }

        #[test]
        fn corrupt_primary_with_valid_snapshot_recovers_newest() {
            let dir = tempfile::tempdir().unwrap();
            seed_snapshots(
                dir.path(),
                &[
                    ("store-100.json", valid_schema_json("older")),
                    ("store-200.json", valid_schema_json("newest")),
                ],
            );
            fs::write(dir.path().join("store.json"), "{broken").unwrap();

            let store = Store::open(dir.path()).unwrap();
            let report = store.recovery.as_ref().expect("must report recovery");
            assert_eq!(report.source, "snapshot:store-200.json", "倒序取最新有效");
            assert_eq!(store.data().active_profile.as_deref(), Some("newest"));
            // 损坏原件可追溯（隔离备份落盘）
            let quarantined = report.quarantined_as.as_ref().unwrap();
            assert!(dir.path().join(quarantined).is_file());
            // 恢复后的主文件与内存一致（下次启动不再走恢复）
            let reopened = Store::open(dir.path()).unwrap();
            assert!(reopened.recovery.is_none());
            assert_eq!(reopened.data().active_profile.as_deref(), Some("newest"));
        }

        #[test]
        fn newest_broken_snapshot_falls_back_to_older_valid_one() {
            let dir = tempfile::tempdir().unwrap();
            seed_snapshots(
                dir.path(),
                &[
                    ("store-100.json", valid_schema_json("older")),
                    ("store-200.json", "{broken-newest".to_string()),
                ],
            );
            fs::write(dir.path().join("store.json"), "{broken").unwrap();

            let store = Store::open(dir.path()).unwrap();
            let report = store.recovery.as_ref().unwrap();
            assert_eq!(report.source, "snapshot:store-100.json");
            assert_eq!(store.data().active_profile.as_deref(), Some("older"));
        }

        #[test]
        fn all_broken_falls_back_to_default_and_keeps_evidence() {
            let dir = tempfile::tempdir().unwrap();
            seed_snapshots(
                dir.path(),
                &[
                    ("store-100.json", "{bad-1".to_string()),
                    ("store-200.json", "{bad-2".to_string()),
                ],
            );
            fs::write(dir.path().join("store.json"), "{broken").unwrap();

            let store = Store::open(dir.path()).unwrap();
            let report = store.recovery.as_ref().unwrap();
            assert_eq!(report.source, "default");
            assert!(store.data().profiles.is_empty());
            // 原件与损坏快照都保留（可追溯、未自动清空成默认库覆盖）
            assert!(report.quarantined_as.is_some());
            assert!(dir
                .path()
                .join("snapshots")
                .join("store-100.json")
                .is_file());
        }

        #[test]
        fn no_snapshot_starts_from_default_with_report() {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join("store.json"), "{broken").unwrap();

            let store = Store::open(dir.path()).unwrap();
            let report = store.recovery.as_ref().unwrap();
            assert_eq!(report.source, "default");
            assert!(report.quarantined_as.is_some());
        }

        /// IO/权限类错误（此处用「store.json 是目录」模拟读失败——文件
        /// 未损坏只是读不了）：不做恢复、明确暴露，不隔离不覆盖。
        #[test]
        fn io_error_kind_surfaces_instead_of_recovering() {
            let dir = tempfile::tempdir().unwrap();
            fs::create_dir(dir.path().join("store.json")).unwrap();
            seed_snapshots(
                dir.path(),
                &[("store-100.json", valid_schema_json("older"))],
            );

            let err = Store::open(dir.path()).unwrap_err();
            assert!(matches!(err, Error::Io(_)));
            // 快照未被消费或覆盖
            assert!(dir
                .path()
                .join("snapshots")
                .join("store-100.json")
                .is_file());
        }

        /// 未来 schema：显示升级/兼容信息，不自动降级写回。
        #[test]
        fn schema_too_new_surfaces_and_never_downgrades() {
            let dir = tempfile::tempdir().unwrap();
            fs::write(
                dir.path().join("store.json"),
                r#"{"schema_version":99,"profiles":[],"overrides":[]}"#,
            )
            .unwrap();
            seed_snapshots(
                dir.path(),
                &[("store-100.json", valid_schema_json("older"))],
            );

            let err = Store::open(dir.path()).unwrap_err();
            assert!(matches!(err, Error::SchemaTooNew { .. }));
            // 主文件原样保留（未被降级写回），快照未被消费
            assert!(dir.path().join("store.json").is_file());
            assert!(dir
                .path()
                .join("snapshots")
                .join("store-100.json")
                .is_file());
        }

        #[test]
        fn future_schema_with_unknown_fields_is_not_quarantined() {
            let dir = tempfile::tempdir().unwrap();
            let raw = r#"{"schema_version":99,"engine":{"engine":"future-engine"}}"#;
            fs::write(dir.path().join("store.json"), raw).unwrap();
            assert!(matches!(
                Store::open(dir.path()),
                Err(Error::SchemaTooNew { .. })
            ));
            assert_eq!(
                fs::read_to_string(dir.path().join("store.json")).unwrap(),
                raw
            );
        }

        #[test]
        fn future_snapshot_blocks_downgrade_and_preserves_primary() {
            let dir = tempfile::tempdir().unwrap();
            seed_snapshots(
                dir.path(),
                &[
                    (
                        "store-200.json",
                        r#"{"schema_version":99,"engine":{"engine":"future-engine"}}"#.into(),
                    ),
                    ("store-100.json", valid_schema_json("older")),
                ],
            );
            fs::write(dir.path().join("store.json"), "{broken").unwrap();
            assert!(matches!(
                Store::open(dir.path()),
                Err(Error::SchemaTooNew { .. })
            ));
            assert_eq!(
                fs::read_to_string(dir.path().join("store.json")).unwrap(),
                "{broken"
            );
        }

        #[test]
        fn recovery_report_does_not_include_invalid_field_values() {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join("store.json"),
                r#"{"schema_version":2,"profiles":[],"overrides":[],"engine":{"engine":"PRIVATE-TOKEN"}}"#).unwrap();
            let store = Store::open(dir.path()).unwrap();
            let report = serde_json::to_string(&store.recovery).unwrap();
            assert!(!report.contains("PRIVATE-TOKEN"));
            assert_eq!(store.recovery.as_ref().unwrap().source, "default");
            assert!(Store::open(dir.path()).unwrap().recovery.is_none());
        }

        #[test]
        fn non_utf8_corruption_recovers_from_snapshot() {
            let dir = tempfile::tempdir().unwrap();
            seed_snapshots(
                dir.path(),
                &[("store-100.json", valid_schema_json("older"))],
            );
            fs::write(dir.path().join("store.json"), [0xff, 0xfe]).unwrap();
            let store = Store::open(dir.path()).unwrap();
            assert_eq!(store.data().active_profile.as_deref(), Some("older"));
            assert!(store.recovery.is_some());
        }

        #[test]
        fn unreadable_snapshot_is_not_treated_as_missing() {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join("store.json"), "{broken").unwrap();
            fs::create_dir_all(dir.path().join("snapshots/store-200.json")).unwrap();
            assert!(matches!(Store::open(dir.path()), Err(Error::Io(_))));
            assert_eq!(
                fs::read_to_string(dir.path().join("store.json")).unwrap(),
                "{broken"
            );
        }

        /// 恢复写回失败（注入 rename 阶段）不破坏原件与快照；重启可重复恢复。
        #[test]
        fn recovery_write_failure_is_retryable_and_non_destructive() {
            let dir = tempfile::tempdir().unwrap();
            seed_snapshots(
                dir.path(),
                &[("store-100.json", valid_schema_json("older"))],
            );
            fs::write(dir.path().join("store.json"), "{broken").unwrap();

            let _inject = crate::secure_write::inject_at(4); // rename 阶段
            let result = Store::open(dir.path());
            drop(_inject);
            // 恢复写回失败 → Store::open 报错（不吞），数据仍在快照里
            assert!(result.is_err());
            let reopened = Store::open(dir.path()).unwrap();
            assert_eq!(reopened.data().active_profile.as_deref(), Some("older"));
            assert!(reopened.recovery.is_some());
            // 原件已被隔离（第一次尝试的正常行为），快照完好——重启可重复恢复
            assert!(dir
                .path()
                .join("snapshots")
                .join("store-100.json")
                .is_file());
        }
    }
}

#[cfg(test)]
mod transaction_tests {
    use super::*;

    fn profile(id: &str) -> crate::Profile {
        crate::Profile {
            id: id.into(),
            name: id.into(),
            kind: crate::ProfileKind::Local,
            url: None,
            content: "proxies: []".into(),
            update_interval_min: 0,
            override_ids: Vec::new(),
            last_updated: None,
            traffic: None,
            last_update: None,
        }
    }

    #[test]
    fn failed_import_preserves_active_profile_and_retry_has_no_ghosts() {
        for create_primary in [false, true] {
            let last_stage = if cfg!(unix) { 5 } else { 4 };
            for stage in 1..=last_stage {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("store.json");
                let mut store = Store::open(dir.path()).unwrap();
                if create_primary {
                    store.save().unwrap();
                }
                let before = serde_json::to_value(store.data()).unwrap();
                let injection = crate::secure_write::inject_path_at(&path, stage);
                assert!(store
                    .transaction(|data| data.add_profile(profile("failed")))
                    .is_err());
                drop(injection);
                assert_eq!(serde_json::to_value(store.data()).unwrap(), before);
                let disk = Store::open(dir.path()).unwrap();
                assert_eq!(serde_json::to_value(disk.data()).unwrap(), before);

                store
                    .transaction(|data| data.add_profile(profile("retry")))
                    .unwrap();
                assert_eq!(store.data().profiles.len(), 1);
                assert_eq!(store.data().active_profile.as_deref(), Some("retry"));
                assert!(store.data().profile("failed").is_none());
                assert_eq!(
                    serde_json::to_value(Store::open(dir.path()).unwrap().data()).unwrap(),
                    serde_json::to_value(store.data()).unwrap()
                );
            }
        }
    }

    #[test]
    fn mutation_error_never_publishes_partial_candidate() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let result: std::result::Result<(), String> = store.transaction(|data| {
            data.add_profile(profile("partial"))?;
            Err("validation failed".into())
        });
        assert!(result.is_err());
        assert!(store.data().profiles.is_empty());
        assert!(!dir.path().join("store.json").exists());
    }

    #[test]
    fn override_bindings_and_preferences_roll_back_together() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.json");
        let mut store = Store::open(dir.path()).unwrap();
        store
            .transaction(|data| data.add_profile(profile("active")))
            .unwrap();
        let before = serde_json::to_value(store.data()).unwrap();
        let injection = crate::secure_write::inject_path_at(&path, 4);
        let result = store.transaction(|data| {
            data.overrides.push(crate::OverrideDef {
                id: "override".into(),
                name: "test".into(),
                kind: crate::OverrideKind::Merge,
                content: "{}".into(),
                enabled: true,
                last_error: None,
            });
            data.profiles[0].override_ids.push("override".into());
            data.ui.lang = "en".into();
            data.logs.save_enabled = !data.logs.save_enabled;
            data.update.auto_check = !data.update.auto_check;
            Ok(())
        });
        drop(injection);
        assert!(result.is_err());
        assert_eq!(serde_json::to_value(store.data()).unwrap(), before);
        assert_eq!(
            serde_json::to_value(Store::open(dir.path()).unwrap().data()).unwrap(),
            before
        );
    }
}
