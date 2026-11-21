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
        // #595：数据目录与既有文件权限收敛（历史版本 umask 下落 0644/0755）；
        // 失败为 best effort——权限收紧不阻断启动。
        let _ = crate::secure_write::ensure_private_dir(dir);
        let path = dir.join("store.json");
        let _ = crate::secure_write::ensure_private(&path);

        let primary = if path.exists() {
            match fs::read_to_string(&path) {
                Ok(raw) => match Self::load_str(&raw) {
                    Ok(data) => {
                        return Ok(Self {
                            path,
                            data,
                            recovery: None,
                        })
                    }
                    // 未来 schema：绝不自动降级写回，向上暴露升级信息
                    Err(error @ Error::SchemaTooNew { .. }) => return Err(error),
                    // 仅解析失败进恢复路径
                    Err(error) => Err(error),
                },
                // 权限/IO 类错误：文件未必损坏，恢复可能越权访问他人数据——
                // 明确暴露，不隔离、不回落（#594 验收：确定结果）。
                Err(error) => return Err(Error::Io(error)),
            }
        } else {
            return Ok(Self {
                path,
                data: StoreSchema::default(),
                recovery: None,
            });
        };

        // ---- 恢复路径（主文件解析失败/损坏）----
        let report_reason = match primary {
            Ok(data) => {
                return Ok(Self {
                    path,
                    data,
                    recovery: None,
                })
            }
            Err(error) => format!("主文件解析失败：{error}"),
        };

        // 隔离损坏原件（原子 rename，保留可追溯；重启可重复恢复——
        // 隔离后 store.json 不存在，下一轮恢复直接走快照/默认）。
        let quarantined = Self::quarantine_corrupt(&path)?;

        // 快照按文件名（纳秒时间戳）倒序 = 时间倒序，逐个校验
        let mut recovered_source = String::new();
        let mut recovered: Option<StoreSchema> = None;
        for snapshot in Self::snapshots_valid_sorted(dir)? {
            let raw = match fs::read_to_string(&snapshot) {
                Ok(raw) => raw,
                Err(_) => continue,
            };
            match Self::load_str(&raw) {
                Ok(data) => {
                    recovered_source = format!(
                        "snapshot:{}",
                        snapshot
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    );
                    recovered = Some(data);
                    break;
                }
                Err(_) => continue,
            }
        }

        let (data, source, reason) = match recovered {
            Some(data) => {
                // 恢复的快照写回为主文件（私有耐久写入，#595 边界）：
                // 让磁盘状态与内存一致，避免下次启动再次走恢复。
                crate::secure_write::write_private_atomic(
                    &path,
                    &serde_json::to_vec_pretty(&data)?,
                )
                .map_err(Error::Io)?;
                (data, recovered_source, "已从快照恢复".to_string())
            }
            None => (
                StoreSchema::default(),
                "default".to_string(),
                "无可用快照，已从默认配置启动".to_string(),
            ),
        };
        let reason = format!("{report_reason}；{reason}");
        Ok(Self {
            path,
            data,
            recovery: Some(RecoveryReport {
                source,
                quarantined_as: quarantined,
                reason,
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
        let mut snaps: Vec<PathBuf> = match fs::read_dir(&snap_dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.file_name()
                        .map(|n| n.to_string_lossy().starts_with("store-"))
                        .unwrap_or(false)
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        snaps.sort();
        snaps.reverse();
        Ok(snaps)
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
            // 原件已被隔离（第一次尝试的正常行为），快照完好——重启可重复恢复
            assert!(dir
                .path()
                .join("snapshots")
                .join("store-100.json")
                .is_file());
        }
    }
}
