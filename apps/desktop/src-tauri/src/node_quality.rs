//! Bounded local observations of user-requested tests. No node names, addresses,
//! credentials or arbitrary target URLs are persisted; no probes run here.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

pub(crate) const MAX_SAMPLES: usize = 8192;
const MAX_BUCKETS: usize = 512;
const PER_BUCKET: usize = 64;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const RETENTION_MS: u64 = 7 * 24 * 3600 * 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Target {
    GoogleHttp,
    GoogleHttps,
    CloudflareHttps,
}
impl Target {
    pub(crate) fn url(self) -> &'static str {
        match self {
            Self::GoogleHttp => "http://www.gstatic.com/generate_204",
            Self::GoogleHttps => "https://www.gstatic.com/generate_204",
            Self::CloudflareHttps => "https://cp.cloudflare.com/generate_204",
        }
    }
    pub(crate) fn from_url(url: &str) -> Option<Self> {
        [Self::GoogleHttp, Self::GoogleHttps, Self::CloudflareHttps]
            .into_iter()
            .find(|t| t.url() == url)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ResultClass {
    Ok,
    Timeout,
    Failed,
    Controller,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Sample {
    pub at_ms: u64,
    pub class: ResultClass,
    pub delay_ms: Option<u64>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Bucket {
    id: String,
    target: Target,
    samples: Vec<Sample>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Document {
    version: u8,
    paused: bool,
    buckets: Vec<Bucket>,
}
impl Default for Document {
    fn default() -> Self {
        Self {
            version: 1,
            paused: false,
            buckets: vec![],
        }
    }
}
struct Inner {
    document: Document,
    epoch: u64,
    dirty: bool,
    load_error: Option<String>,
    save_error: Option<String>,
}
pub(crate) struct History {
    path: PathBuf,
    inner: Mutex<Inner>,
}
#[derive(Clone)]
pub(crate) struct Scope(String);
impl Scope {
    pub(crate) fn from_store(data: &crossbow_core::StoreSchema) -> Option<Self> {
        let profile = data
            .active_profile
            .as_ref()
            .and_then(|id| data.profile(id))?;
        let overrides: Vec<_> = data
            .overrides
            .iter()
            .filter(|o| profile.override_ids.contains(&o.id))
            .collect();
        let value = serde_json::json!({"profile":profile.id,"order":profile.override_ids,"content":profile.content,"engine":data.engine.engine,"overrides":overrides,"dns":data.dns});
        let bytes = serde_json::to_vec(&value).ok()?;
        Some(Self(format!("{:x}", Sha256::digest(bytes))))
    }
    fn node(&self, node: &str) -> String {
        let mut hash = Sha256::new();
        hash.update(self.0.as_bytes());
        hash.update([0]);
        hash.update(node.as_bytes());
        format!("{:x}", hash.finalize())
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Summary {
    target: Target,
    total: usize,
    eligible: usize,
    successes: usize,
    timeouts: usize,
    failures: usize,
    controller_errors: usize,
    success_rate: Option<f64>,
    p50_ms: Option<u64>,
    p95_ms: Option<u64>,
    jitter_ms: Option<u64>,
    samples: Vec<Sample>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Report {
    pub paused: bool,
    pub epoch: u64,
    retention_days: u32,
    capacity: usize,
    stored_samples: usize,
    pub storage_error: Option<String>,
    pub test_targets: Vec<Target>,
    pending_save: bool,
    targets: Vec<Summary>,
}
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn prune(document: &mut Document, now: u64) {
    for bucket in &mut document.buckets {
        bucket.samples.retain(|sample| {
            sample.at_ms <= now.saturating_add(60_000)
                && now.saturating_sub(sample.at_ms) <= RETENTION_MS
                && (sample.class != ResultClass::Ok
                    || sample.delay_ms.is_some_and(|ms| ms > 0 && ms <= 120_000))
        });
        bucket.samples.sort_by_key(|sample| sample.at_ms);
        if bucket.samples.len() > PER_BUCKET {
            bucket.samples.drain(..bucket.samples.len() - PER_BUCKET);
        }
    }
    document.buckets.retain(|b| !b.samples.is_empty());
    document
        .buckets
        .sort_by_key(|b| b.samples.last().map(|s| s.at_ms).unwrap_or(0));
    if document.buckets.len() > MAX_BUCKETS {
        document
            .buckets
            .drain(..document.buckets.len() - MAX_BUCKETS);
    }
    let mut count: usize = document.buckets.iter().map(|b| b.samples.len()).sum();
    while count > MAX_SAMPLES {
        let Some((index, _)) = document
            .buckets
            .iter()
            .enumerate()
            .filter_map(|(i, b)| b.samples.first().map(|s| (i, s.at_ms)))
            .min_by_key(|(_, at)| *at)
        else {
            break;
        };
        document.buckets[index].samples.remove(0);
        count -= 1;
    }
    document.buckets.retain(|b| !b.samples.is_empty());
}
fn summary(target: Target, samples: Vec<Sample>) -> Summary {
    let successful: Vec<_> = samples
        .iter()
        .filter(|s| s.class == ResultClass::Ok)
        .filter_map(|s| s.delay_ms)
        .collect();
    let mut sorted = successful.clone();
    sorted.sort_unstable();
    let percentile = |n: usize| {
        (!sorted.is_empty()).then(|| sorted[(sorted.len() * n).div_ceil(100).saturating_sub(1)])
    };
    let jitter = (successful.len() > 1)
        .then(|| {
            successful
                .windows(2)
                .map(|w| w[0].abs_diff(w[1]) as u128)
                .sum::<u128>()
                / (successful.len() - 1) as u128
        })
        .map(|v| v as u64);
    let count = |class| samples.iter().filter(|s| s.class == class).count();
    let eligible = samples.len() - count(ResultClass::Controller);
    Summary {
        target,
        total: samples.len(),
        eligible,
        successes: successful.len(),
        timeouts: count(ResultClass::Timeout),
        failures: count(ResultClass::Failed),
        controller_errors: count(ResultClass::Controller),
        success_rate: (eligible > 0).then(|| successful.len() as f64 / eligible as f64),
        p50_ms: percentile(50),
        p95_ms: percentile(95),
        jitter_ms: jitter,
        samples: samples.into_iter().rev().take(12).collect(),
    }
}
fn read_bounded(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let metadata = path.symlink_metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return Err(std::io::Error::other("invalid quality history file"));
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("invalid quality history file"));
    }
    let mut raw = vec![];
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut raw)?;
    if raw.len() as u64 > MAX_FILE_BYTES {
        return Err(std::io::Error::other("quality history too large"));
    }
    Ok(raw)
}

impl History {
    pub(crate) fn open(dir: &Path) -> Self {
        let path = dir.join("node-quality.json");
        let loaded = (|| {
            let raw = read_bounded(&path)?;
            let data: Document = serde_json::from_slice(&raw).map_err(std::io::Error::other)?;
            if data.version != 1 {
                return Err(std::io::Error::other("unsupported quality history version"));
            }
            Ok(data)
        })();
        let (document, load_error) = match loaded {
            Ok(data) => (data, None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                (Document::default(), None)
            }
            Err(_) => (
                Document::default(),
                Some("质量历史无法读取，原文件已保留；清空历史后可重新记录".into()),
            ),
        };
        let mut document = document;
        let before: usize = document.buckets.iter().map(|b| b.samples.len()).sum();
        prune(&mut document, now_ms());
        let after: usize = document.buckets.iter().map(|b| b.samples.len()).sum();
        let history = Self {
            path,
            inner: Mutex::new(Inner {
                document,
                epoch: 0,
                dirty: before != after,
                load_error,
                save_error: None,
            }),
        };
        // Retention applies at startup, even when the Nodes page is never opened.
        // Failed writes remain observable via save_error/pending_save.
        let _ = history.flush();
        history
    }
    pub(crate) fn epoch(&self) -> u64 {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).epoch
    }
    pub(crate) fn record(
        &self,
        epoch: u64,
        scope: &Scope,
        node: &str,
        target: Target,
        sample: Sample,
    ) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.epoch != epoch || inner.document.paused || inner.load_error.is_some() {
            return;
        }
        let id = scope.node(node);
        let index = inner
            .document
            .buckets
            .iter()
            .position(|b| b.id == id && b.target == target)
            .unwrap_or_else(|| {
                inner.document.buckets.push(Bucket {
                    id,
                    target,
                    samples: vec![],
                });
                inner.document.buckets.len() - 1
            });
        inner.document.buckets[index].samples.push(sample);
        prune(&mut inner.document, now_ms());
        inner.dirty = true;
    }
    fn save(&self, data: &Document) -> Result<(), String> {
        let raw = serde_json::to_vec(data).map_err(|e| e.to_string())?;
        if raw.len() as u64 > MAX_FILE_BYTES {
            return Err("质量历史超过存储上限".into());
        }
        crate::platform::fs_perms::write_private(&self.path, &raw)
            .map_err(|_| "质量历史保存失败，仅保留本次会话数据".into())
    }
    pub(crate) fn flush(&self) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(error) = &inner.load_error {
            return Err(error.clone());
        }
        self.flush_locked(&mut inner)
    }
    fn flush_locked(&self, inner: &mut Inner) -> Result<(), String> {
        if inner.dirty {
            let result = self.save(&inner.document);
            inner.save_error = result.as_ref().err().cloned();
            result?;
            inner.dirty = false;
        }
        Ok(())
    }
    fn replace(&self, inner: &mut Inner, data: Document) -> Result<(), String> {
        // Every privacy action invalidates pending observations, including a failed write.
        inner.epoch = inner.epoch.wrapping_add(1);
        let result = self.save(&data);
        inner.save_error = result.as_ref().err().cloned();
        // Atomic rename may have committed before a directory-sync error. Reconcile
        // the in-memory preference with the actual file, while still reporting the error.
        let committed = result.is_ok()
            || read_bounded(&self.path)
                .ok()
                .is_some_and(|raw| serde_json::to_vec(&data).ok().as_ref() == Some(&raw));
        if committed {
            inner.document = data;
            inner.dirty = false;
            inner.load_error = None;
        }
        result
    }
    pub(crate) fn set_paused(&self, paused: bool) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(error) = &inner.load_error {
            return Err(error.clone());
        }
        let mut data = inner.document.clone();
        data.paused = paused;
        self.replace(&mut inner, data)
    }
    pub(crate) fn clear(&self) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let data = Document {
            paused: inner.document.paused,
            ..Default::default()
        };
        self.replace(&mut inner, data)
    }
    pub(crate) fn report(&self, scope: Option<&Scope>, node: &str) -> Report {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let before: usize = inner.document.buckets.iter().map(|b| b.samples.len()).sum();
        prune(&mut inner.document, now_ms());
        let after: usize = inner.document.buckets.iter().map(|b| b.samples.len()).sum();
        inner.dirty |= before != after;
        if inner.load_error.is_none() {
            let _ = self.flush_locked(&mut inner);
        }
        let id = scope.map(|s| s.node(node));
        let targets = [
            Target::GoogleHttp,
            Target::GoogleHttps,
            Target::CloudflareHttps,
        ]
        .into_iter()
        .map(|target| {
            let samples = inner
                .document
                .buckets
                .iter()
                .find(|b| Some(&b.id) == id.as_ref() && b.target == target)
                .map(|b| b.samples.clone())
                .unwrap_or_default();
            summary(target, samples)
        })
        .collect();
        Report {
            test_targets: vec![
                Target::GoogleHttp,
                Target::GoogleHttps,
                Target::CloudflareHttps,
            ],
            paused: inner.document.paused,
            epoch: inner.epoch,
            retention_days: 7,
            capacity: MAX_SAMPLES,
            stored_samples: inner.document.buckets.iter().map(|b| b.samples.len()).sum(),
            storage_error: inner
                .load_error
                .clone()
                .or_else(|| inner.save_error.clone()),
            pending_save: inner.dirty,
            targets,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(class: ResultClass, ms: Option<u64>) -> Sample {
        Sample {
            at_ms: now_ms(),
            class,
            delay_ms: ms,
        }
    }
    fn scope() -> Scope {
        Scope("fixture-profile".into())
    }
    #[test]
    fn scope_tracks_configuration_engine_and_bound_override_order_without_storing_content() {
        let mut data = crossbow_core::StoreSchema::default();
        data.add_profile(serde_json::from_value(serde_json::json!({"id":"fixture","name":"private name","kind":"local","content":"server: secret.invalid\npassword: synthetic-secret", "update_interval_min":0,"override_ids":[]})).unwrap()).unwrap();
        data.set_active_profile("fixture").unwrap();
        let scope = Scope::from_store(&data).unwrap();
        data.profiles[0].name = "renamed label".into();
        assert_eq!(scope.0, Scope::from_store(&data).unwrap().0);
        data.profiles[0].content.push_str("\nport: 1234");
        assert_ne!(scope.0, Scope::from_store(&data).unwrap().0);
        let previous = Scope::from_store(&data).unwrap();
        data.engine.engine = crossbow_core::Engine::SingBox;
        assert_ne!(previous.0, Scope::from_store(&data).unwrap().0);
        let previous = Scope::from_store(&data).unwrap();
        data.profiles[0].override_ids = vec!["first".into(), "second".into()];
        let ordered = Scope::from_store(&data).unwrap();
        assert_ne!(previous.0, ordered.0);
        data.profiles[0].override_ids.reverse();
        assert_ne!(ordered.0, Scope::from_store(&data).unwrap().0);
        let dir = tempfile::tempdir().unwrap();
        let h = History::open(dir.path());
        h.record(
            0,
            &scope,
            "node",
            Target::GoogleHttp,
            sample(ResultClass::Ok, Some(20)),
        );
        h.flush().unwrap();
        let raw = std::fs::read_to_string(dir.path().join("node-quality.json")).unwrap();
        assert!(!raw.contains("synthetic-secret"));
        assert!(!raw.contains("secret.invalid"));
        assert!(!raw.contains("private name"));
    }

    #[test]
    fn statistics_exclude_controller_and_keep_target_failures() {
        let summary = summary(
            Target::GoogleHttp,
            vec![
                sample(ResultClass::Ok, Some(10)),
                sample(ResultClass::Timeout, None),
                sample(ResultClass::Ok, Some(90)),
                sample(ResultClass::Failed, None),
                sample(ResultClass::Controller, None),
            ],
        );
        assert_eq!(summary.eligible, 4);
        assert_eq!(summary.success_rate, Some(0.5));
        assert_eq!(summary.p50_ms, Some(10));
        assert_eq!(summary.p95_ms, Some(90));
        assert_eq!(summary.jitter_ms, Some(80));
        assert_eq!(summary.controller_errors, 1);
        let empty = super::summary(
            Target::GoogleHttp,
            vec![sample(ResultClass::Controller, None)],
        );
        assert_eq!(empty.success_rate, None);
        assert_eq!(empty.p50_ms, None);
        assert_eq!(empty.jitter_ms, None);
        assert_eq!(
            super::summary(Target::GoogleHttp, vec![sample(ResultClass::Ok, Some(10))]).jitter_ms,
            None
        );
    }
    #[test]
    fn privacy_and_scope_target_isolation_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::open(dir.path());
        history.record(
            0,
            &scope(),
            "secret-node-password",
            Target::GoogleHttp,
            sample(ResultClass::Timeout, None),
        );
        history.record(
            0,
            &scope(),
            "secret-node-password",
            Target::CloudflareHttps,
            sample(ResultClass::Ok, Some(80)),
        );
        history.flush().unwrap();
        let raw = std::fs::read_to_string(dir.path().join("node-quality.json")).unwrap();
        for private in [
            "secret-node-password",
            "fixture-profile",
            "https://",
            "password",
        ] {
            assert!(!raw.contains(private));
        }
        let loaded = History::open(dir.path());
        let report = loaded.report(Some(&scope()), "secret-node-password");
        assert_eq!(report.targets[0].timeouts, 1);
        assert_eq!(report.targets[2].successes, 1);
        assert!(loaded
            .report(Some(&Scope("other-profile".into())), "secret-node-password")
            .targets
            .iter()
            .all(|s| s.total == 0));
        assert!(loaded
            .report(Some(&scope()), "different-node")
            .targets
            .iter()
            .all(|s| s.total == 0));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                dir.path()
                    .join("node-quality.json")
                    .metadata()
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn pause_clear_and_resume_fence_late_results() {
        let dir = tempfile::tempdir().unwrap();
        let h = History::open(dir.path());
        let add = |epoch| {
            h.record(
                epoch,
                &scope(),
                "node",
                Target::GoogleHttp,
                sample(ResultClass::Ok, Some(20)),
            )
        };
        add(0);
        h.set_paused(true).unwrap();
        add(0);
        add(h.epoch());
        assert_eq!(h.report(Some(&scope()), "node").stored_samples, 1);
        h.set_paused(false).unwrap();
        add(0);
        assert_eq!(h.report(Some(&scope()), "node").stored_samples, 1);
        let epoch = h.epoch();
        h.clear().unwrap();
        add(epoch);
        assert_eq!(h.report(Some(&scope()), "node").stored_samples, 0);
        assert_eq!(
            History::open(dir.path())
                .report(Some(&scope()), "node")
                .stored_samples,
            0
        );
        add(h.epoch());
        assert_eq!(h.report(Some(&scope()), "node").stored_samples, 1);
    }
    #[test]
    fn bounded_by_retention_bucket_and_global_capacity() {
        let now = now_ms();
        let mut doc = Document::default();
        for i in 0..600 {
            doc.buckets.push(Bucket {
                id: i.to_string(),
                target: Target::GoogleHttp,
                samples: (0..80)
                    .map(|n| Sample {
                        at_ms: now - 1000 + n,
                        class: ResultClass::Ok,
                        delay_ms: Some(20),
                    })
                    .collect(),
            });
        }
        doc.buckets[0].samples.push(Sample {
            at_ms: now - RETENTION_MS - 1,
            class: ResultClass::Timeout,
            delay_ms: None,
        });
        prune(&mut doc, now);
        assert!(doc.buckets.len() <= MAX_BUCKETS);
        assert!(doc.buckets.iter().all(|b| b.samples.len() <= PER_BUCKET));
        assert_eq!(
            doc.buckets.iter().map(|b| b.samples.len()).sum::<usize>(),
            MAX_SAMPLES
        );
        assert!(serde_json::to_vec(&doc).unwrap().len() as u64 <= MAX_FILE_BYTES);
        prune(&mut doc, now + RETENTION_MS + 1);
        assert!(doc.buckets.is_empty());
    }
    #[test]
    fn expired_samples_are_removed_from_disk_on_startup_and_query() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node-quality.json");
        let data = Document {
            buckets: vec![Bucket {
                id: scope().node("node"),
                target: Target::GoogleHttp,
                samples: vec![Sample {
                    at_ms: now_ms() - RETENTION_MS - 1,
                    class: ResultClass::Timeout,
                    delay_ms: None,
                }],
            }],
            ..Default::default()
        };
        std::fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
        let h = History::open(dir.path());
        let startup: Document = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(startup.buckets.is_empty());
        assert_eq!(h.report(Some(&scope()), "node").stored_samples, 0);
        let disk: Document = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(disk.buckets.is_empty());
    }
    #[cfg(unix)]
    #[test]
    fn load_rejects_symlinks_and_nonregular_files_without_touching_target() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("other");
        std::fs::write(&target, b"private source").unwrap();
        let path = dir.path().join("node-quality.json");
        symlink(&target, &path).unwrap();
        let h = History::open(dir.path());
        assert!(h.report(None, "").storage_error.is_some());
        assert!(h.clear().is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"private source");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(History::open(dir.path())
            .report(None, "")
            .storage_error
            .is_some());
    }

    #[test]
    fn corrupt_file_is_preserved_until_explicit_clear() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node-quality.json");
        std::fs::write(&path, b"invalid").unwrap();
        let h = History::open(dir.path());
        h.record(
            0,
            &scope(),
            "node",
            Target::GoogleHttp,
            sample(ResultClass::Ok, Some(1)),
        );
        assert!(h.flush().is_err());
        assert!(h.set_paused(true).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"invalid");
        assert!(h.report(None, "").storage_error.is_some());
        h.clear().unwrap();
        assert!(h.report(None, "").storage_error.is_none());
    }
    #[test]
    fn failed_persistence_keeps_real_preference_and_exposes_unsaved_data() {
        let dir = tempfile::tempdir().unwrap();
        let h = History::open(dir.path());
        std::fs::create_dir(dir.path().join("node-quality.json")).unwrap();
        h.record(
            0,
            &scope(),
            "node",
            Target::GoogleHttp,
            sample(ResultClass::Ok, Some(10)),
        );
        assert!(h.set_paused(true).is_err());
        let r = h.report(Some(&scope()), "node");
        assert!(!r.paused);
        assert!(r.pending_save);
        assert!(r.storage_error.is_some());
        assert!(h.clear().is_err());
        assert_eq!(h.report(Some(&scope()), "node").stored_samples, 1);
        h.record(
            0,
            &scope(),
            "node",
            Target::GoogleHttp,
            sample(ResultClass::Ok, Some(99)),
        );
        assert_eq!(h.report(Some(&scope()), "node").stored_samples, 1);
    }
}
