//! Explicit, single-flight finite measurements. No automatic retries, routing changes or raw logs.
use crossbow_core::Engine;
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    Connectivity,
    Throughput,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Check {
    pub class: String,
    pub elapsed_ms: u64,
    pub packets: u8,
}
impl Check {
    pub fn ok(elapsed: Duration, packets: u8) -> Self {
        Self {
            class: "ok".into(),
            elapsed_ms: elapsed.as_millis() as u64,
            packets,
        }
    }
    pub fn unknown(class: &str, elapsed: Duration, packets: u8) -> Self {
        Self {
            class: class.into(),
            elapsed_ms: elapsed.as_millis() as u64,
            packets,
        }
    }
    fn untested() -> Self {
        Self::unknown("not_tested", Duration::ZERO, 0)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Transfer {
    pub class: String,
    pub bytes: u64,
    pub elapsed_ms: u64,
    pub limit_bytes: u64,
    pub mbps: Option<f64>,
}
impl Transfer {
    pub fn new(ok: bool, bytes: u64, elapsed: Duration, limit_bytes: u64) -> Self {
        let mbps = (ok && elapsed.as_secs_f64() > 0.).then(|| {
            ((bytes as f64 * 8. / elapsed.as_secs_f64() / 1_000_000.) * 100.).round() / 100.
        });
        Self {
            class: if ok { "ok" } else { "incomplete" }.into(),
            bytes,
            elapsed_ms: elapsed.as_millis() as u64,
            limit_bytes,
            mbps,
        }
    }
    fn untested() -> Self {
        Self {
            class: "not_tested".into(),
            bytes: 0,
            elapsed_ms: 0,
            limit_bytes: 0,
            mbps: None,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Report {
    pub kind: Kind,
    pub at_ms: u64,
    pub elapsed_ms: u64,
    pub tcp: Check,
    pub https: Check,
    pub udp: Check,
    pub download: Transfer,
    pub upload: Transfer,
}
impl Report {
    pub(crate) fn empty(kind: Kind) -> Self {
        Self {
            kind,
            at_ms: crate::node_quality::now_ms(),
            elapsed_ms: 0,
            tcp: Check::untested(),
            https: Check::untested(),
            udp: Check::untested(),
            download: Transfer::untested(),
            upload: Transfer::untested(),
        }
    }
}
#[derive(Clone)]
pub(crate) struct Budget {
    deadline: Instant,
    cancel: Arc<AtomicBool>,
    current: Arc<dyn Fn() -> bool + Send + Sync>,
}
impl Budget {
    pub fn new(current: Arc<dyn Fn() -> bool + Send + Sync>) -> Self {
        Self {
            deadline: Instant::now() + Duration::from_secs(30),
            cancel: Arc::new(AtomicBool::new(false)),
            current,
        }
    }
    pub fn check(&self) -> Result<(), String> {
        if self.cancelled() {
            Err("cancelled".into())
        } else if Instant::now() >= self.deadline {
            Err("expired".into())
        } else {
            Ok(())
        }
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst) || !(self.current)()
    }
    pub fn remaining(&self) -> Result<Duration, String> {
        self.check()?;
        Ok(self.deadline.saturating_duration_since(Instant::now()))
    }
    pub fn child(&self, max: Duration) -> Self {
        Self {
            deadline: self.deadline.min(Instant::now() + max),
            ..self.clone()
        }
    }
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    pub id: String,
    pub node: String,
    pub kind: Kind,
    pub state: String,
    pub phase: String,
    pub report: Option<Report>,
    pub error: Option<String>,
}
struct Job {
    snapshot: Snapshot,
    budget: Budget,
}
#[derive(Default)]
pub(crate) struct Service {
    inner: Mutex<Option<Job>>,
}
pub(crate) struct Input {
    pub binary: PathBuf,
    pub config: String,
    pub engine: Engine,
    pub node: String,
    pub kind: Kind,
}
type Finish = Arc<dyn Fn(&Snapshot) + Send + Sync>;
impl Service {
    pub fn busy(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|j| matches!(j.snapshot.state.as_str(), "running" | "cancelling"))
    }
    pub fn snapshot(&self) -> Option<Snapshot> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|j| j.snapshot.clone())
    }
    pub fn cancel(&self, id: &str) {
        if let Some(job) = self
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
            .filter(|j| {
                j.snapshot.id == id && matches!(j.snapshot.state.as_str(), "running" | "cancelling")
            })
        {
            job.budget.cancel.store(true, Ordering::SeqCst);
            job.snapshot.state = "cancelling".into();
        }
    }
    fn phase(&self, id: &str, value: &str, event: &Finish) {
        let snapshot = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let Some(job) = inner.as_mut().filter(|j| j.snapshot.id == id) else {
                return;
            };
            job.snapshot.phase = value.into();
            job.snapshot.clone()
        };
        event(&snapshot);
    }
    pub fn begin(
        self: &Arc<Self>,
        input: Input,
        current: Arc<dyn Fn() -> bool + Send + Sync>,
        finish: Finish,
    ) -> Result<String, String> {
        let mut active = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if active
            .as_ref()
            .is_some_and(|j| matches!(j.snapshot.state.as_str(), "running" | "cancelling"))
        {
            return Err("已有分项测试正在执行或回收，请稍后重试".into());
        }
        let budget = Budget::new(current);
        budget.check()?;
        let id = uuid::Uuid::new_v4().to_string();
        *active = Some(Job {
            snapshot: Snapshot {
                id: id.clone(),
                node: input.node.clone(),
                kind: input.kind,
                state: "running".into(),
                phase: "preparing".into(),
                report: None,
                error: None,
            },
            budget: budget.clone(),
        });
        let service = self.clone();
        let task = id.clone();
        let events = finish.clone();
        let thread = std::thread::Builder::new()
            .name("node-probe".into())
            .spawn(move || {
                let start = Instant::now();
                let mut report = Report::empty(input.kind);
                let result = (|| {
                    let worker = crate::probe_worker::Worker::start(
                        &input.binary,
                        &input.config,
                        input.engine,
                        &input.node,
                        &budget,
                    )?;
                    use crate::probe_worker::{http, transport};
                    if input.kind == Kind::Connectivity {
                        service.phase(&task, "tcp", &events);
                        report.tcp = transport::tcp(
                            worker.port,
                            &worker.password,
                            &budget.child(Duration::from_secs(5)),
                        );
                        budget.check()?;
                        service.phase(&task, "https", &events);
                        report.https = http::https(
                            worker.port,
                            &worker.password,
                            &budget.child(Duration::from_secs(7)),
                        );
                        budget.check()?;
                        service.phase(&task, "udp", &events);
                        report.udp = if worker.udp_supported {
                            transport::udp(
                                worker.port,
                                &worker.password,
                                &budget.child(Duration::from_secs(4)),
                            )
                        } else {
                            Check::unknown("unsupported", Duration::ZERO, 0)
                        };
                    } else {
                        service.phase(&task, "download", &events);
                        report.download = http::download(
                            worker.port,
                            &worker.password,
                            &budget.child(Duration::from_secs(8)),
                        );
                        budget.check()?;
                        service.phase(&task, "upload", &events);
                        report.upload = http::upload(
                            worker.port,
                            &worker.password,
                            &budget.child(Duration::from_secs(6)),
                        );
                    }
                    budget.check()
                    // Worker is killed/reaped before a new task can start or final results are published.
                })();
                report.elapsed_ms = start.elapsed().as_millis() as u64;
                {
                    let mut state = service.inner.lock().unwrap_or_else(|e| e.into_inner());
                    let Some(job) = state.as_mut().filter(|j| j.snapshot.id == task) else {
                        return;
                    };
                    job.snapshot.phase = "finished".into();
                    if budget.cancelled() {
                        job.snapshot.state = "cancelled".into();
                        job.snapshot.report = None;
                    } else if Instant::now() >= budget.deadline {
                        job.snapshot.state = "expired".into();
                        job.snapshot.report = Some(report);
                    } else if let Err(error) = result {
                        job.snapshot.state = "failed".into();
                        job.snapshot.error = Some(error);
                    } else {
                        job.snapshot.state = "completed".into();
                        job.snapshot.report = Some(report);
                    }
                    // Release the AppHandle-bearing validity closure after terminal state.
                    job.budget.current = Arc::new(|| false);
                    // Event handler persists the result before readers can observe completion.
                    // It must not re-enter this Service.
                    events(&job.snapshot);
                }
            });
        if thread.is_err() {
            *active = None;
            return Err("无法启动分项测试任务".into());
        }
        if let Some(job) = active.as_ref() {
            finish(&job.snapshot);
        }
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_and_expired_budgets_stop_new_work() {
        let b = Budget::new(Arc::new(|| true));
        assert!(b.check().is_ok());
        let expired = b.child(Duration::ZERO);
        assert_eq!(expired.check().unwrap_err(), "expired");
        b.cancel.store(true, Ordering::SeqCst);
        assert_eq!(b.check().unwrap_err(), "cancelled");
        assert!(Budget::new(Arc::new(|| false)).check().is_err());
    }
    #[test]
    fn failed_task_releases_validity_owner_and_can_be_replaced() {
        let service = Arc::new(Service::default());
        let current: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(|| true);
        let weak = Arc::downgrade(&current);
        let input = || Input {
            binary: PathBuf::from("missing-test-core"),
            config: "{}".into(),
            engine: Engine::Mihomo,
            node: "fixture".into(),
            kind: Kind::Connectivity,
        };
        let id = service.begin(input(), current, Arc::new(|_| {})).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while (service.busy() || weak.upgrade().is_some()) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!service.busy());
        assert!(weak.upgrade().is_none());
        assert_eq!(service.snapshot().unwrap().state, "failed");
        service.cancel(&id);
        assert_eq!(service.snapshot().unwrap().state, "failed");
        assert!(service
            .begin(input(), Arc::new(|| true), Arc::new(|_| {}))
            .is_ok());
    }

    #[test]
    fn partial_or_zero_duration_transfer_never_reports_a_success_rate() {
        assert!(Transfer::new(false, 123, Duration::from_secs(1), 999)
            .mbps
            .is_none());
        assert!(Transfer::new(true, 123, Duration::ZERO, 123).mbps.is_none());
        assert_eq!(
            Transfer::new(true, 1_000_000, Duration::from_secs(1), 1_000_000).mbps,
            Some(8.)
        );
    }
}

#[cfg(test)]
mod native_suite {
    use super::*;
    /// Explicitly opt in: fixed public Cloudflare targets, <=5 MiB payload/30s, private DIRECT-only fixture.
    #[test]
    #[ignore = "requires CB_PROBE_CORE/HELPER and explicit public test output"]
    fn native_bounded_public_component_suite() {
        let binary = std::env::var("CB_PROBE_CORE").expect("CB_PROBE_CORE");
        let engine = if std::env::var("CB_PROBE_ENGINE").as_deref() == Ok("singbox") {
            Engine::SingBox
        } else {
            Engine::Mihomo
        };
        let kind = if std::env::var("CB_PROBE_KIND").as_deref() == Ok("throughput") {
            Kind::Throughput
        } else {
            Kind::Connectivity
        };
        let output = std::env::var("CB_PROBE_OUTPUT").expect("explicit output path");
        let source = if engine == Engine::Mihomo {
            serde_json::json!({"proxies":[{"name":"fixture,direct","type":"direct"}]})
        } else {
            serde_json::json!({"outbounds":[{"tag":"fixture,direct","type":"direct"}]})
        };
        let service = Arc::new(Service::default());
        let path = output.clone();
        let id = service
            .begin(
                Input {
                    binary: binary.into(),
                    config: source.to_string(),
                    engine,
                    node: "fixture,direct".into(),
                    kind,
                },
                Arc::new(|| true),
                Arc::new(move |s| {
                    std::fs::write(&path, serde_json::to_vec_pretty(s).unwrap()).unwrap();
                }),
            )
            .unwrap();
        let end = Instant::now() + Duration::from_secs(35);
        while service.busy() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            !service.busy(),
            "finite task must finish within deadline and cleanup margin"
        );
        let final_state = service.snapshot().unwrap();
        assert_eq!(final_state.id, id);
        assert_ne!(
            final_state.state, "failed",
            "public test fixture must start correctly: {:?}",
            final_state.error
        );
        let report = final_state.report.unwrap();
        assert!(report.download.bytes <= crate::probe_worker::http::DOWNLOAD_BYTES);
        assert!(report.upload.bytes <= crate::probe_worker::http::UPLOAD_BYTES);
        assert!(report.udp.packets <= 3);
        println!(
            "native component evidence: engine={engine:?}, kind={kind:?}, elapsed_ms={}, state={}",
            report.elapsed_ms, final_state.state
        );
    }
}
