//! macOS 系统代理故障现场：低频 HTTPS 金丝雀、单轮分层证据、观察模式（#488）。
//! 不切节点、不重启内核、不修改系统代理。主探针及独立复核分别留存。

#[cfg(any(target_os = "macos", test))]
use std::io::{Read, Write};
#[cfg(target_os = "macos")]
use std::net::TcpStream;
#[cfg(any(target_os = "macos", test))]
use std::path::Path;
#[cfg(any(target_os = "macos", test))]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "macos")]
use std::sync::{LazyLock, Mutex};
#[cfg(any(target_os = "macos", test))]
use std::time::{Duration, Instant};

#[cfg(any(target_os = "macos", test))]
use serde::{Deserialize, Serialize};
#[cfg(target_os = "macos")]
use sha2::{Digest, Sha256};
#[cfg(target_os = "macos")]
use tauri::{Emitter, Manager};

use crate::AppState;

#[cfg(target_os = "macos")]
pub const CANARY_INTERVAL_TICKS: u32 = 24;
#[cfg(any(target_os = "macos", test))]
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
#[cfg(target_os = "macos")]
const ROUND_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(any(target_os = "macos", test))]
const MAX_RECORD_BYTES: u64 = 16 * 1024;
#[cfg(target_os = "macos")]
const DIRECT_CN: [&str; 2] = [
    "http://connect.rom.miui.com/generate_204",
    "http://wifi.vivo.com.cn/generate_204",
];
#[cfg(any(target_os = "macos", test))]
const DIRECT_GLOBAL: [&str; 2] = [
    "https://www.gstatic.com/generate_204",
    "https://cp.cloudflare.com/generate_204",
];
#[cfg(any(target_os = "macos", test))]
const PROXY_TARGETS: [&str; 2] = DIRECT_GLOBAL;

#[cfg(target_os = "macos")]
static ROUND_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "macos")]
static OBSERVATION_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
// 发布与配置/接管入口共用此锁：关开或改回原值也会使旧结果失效（防 ABA）。
#[cfg(target_os = "macos")]
static REVISION: Mutex<u64> = Mutex::new(0);
#[cfg(target_os = "macos")]
static SESSION: LazyLock<String> = LazyLock::new(|| uuid::Uuid::new_v4().to_string());

pub(crate) fn invalidate() {
    #[cfg(target_os = "macos")]
    {
        let mut revision = REVISION.lock().unwrap_or_else(|e| e.into_inner());
        *revision = revision.wrapping_add(1);
    }
}

#[cfg(any(target_os = "macos", test))]
struct Flight<'a>(&'a AtomicBool);
#[cfg(any(target_os = "macos", test))]
impl<'a> Flight<'a> {
    fn enter(flag: &'a AtomicBool) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| Self(flag))
    }
}
#[cfg(any(target_os = "macos", test))]
impl Drop for Flight<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeOutcome {
    pub class: String,
    pub http_status: Option<u16>,
}
#[cfg(any(target_os = "macos", test))]
impl ProbeOutcome {
    fn unknown() -> Self {
        Self {
            class: "unknown".into(),
            http_status: None,
        }
    }
    pub(crate) fn is_ok(&self) -> bool {
        self.class == "ok"
    }
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeEvidence {
    pub primary: ProbeOutcome,
    pub review: Option<ProbeOutcome>,
}
#[cfg(any(target_os = "macos", test))]
impl ProbeEvidence {
    fn reachable(&self) -> bool {
        self.primary.is_ok() || self.review.as_ref().is_some_and(ProbeOutcome::is_ok)
    }
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Generation {
    session: String,
    revision: u64,
    profile_id: Option<String>,
    mixed_port: u16,
    core_pid: Option<u32>,
    tun_enabled: bool,
    sysproxy_enabled: bool,
    config_digest: String,
    network_context: Option<String>,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaultEvidence {
    pub mode: String,
    pub started_at_ms: u64,
    pub resolved_at_ms: Option<u64>,
    generation: Generation,
    pub profile_id: Option<String>,
    pub mixed_port: u16,
    pub sysproxy_observed: Vec<String>,
    pub mixed_port_ok: bool,
    pub controller: ProbeOutcome,
    // 不经过显式代理不等于物理直连，其他隧道/路由未被证明排除。
    pub direct_path: String,
    pub direct_cn: ProbeEvidence,
    pub direct_global: ProbeEvidence,
    pub proxy_https: ProbeEvidence,
}

#[cfg(any(target_os = "macos", test))]
fn classify_status(status: u16, expected_status: u16) -> &'static str {
    if status == expected_status {
        "ok"
    } else {
        "http_status"
    }
}

#[cfg(any(target_os = "macos", test))]
pub fn classify_transport(text: &str) -> &'static str {
    let low = text.to_lowercase();
    if low.contains("timed out") || low.contains("deadline") || low.contains("timeout") {
        "timeout"
    } else if low.contains("certificate") || low.contains("tls") || low.contains("ssl") {
        "tls"
    } else if low.contains("dns") {
        "dns"
    } else if low.contains("refused") || low.contains("connect") {
        "connect"
    } else {
        "unknown"
    }
}

#[cfg(any(target_os = "macos", test))]
fn budget(deadline: Instant) -> Option<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
}

#[cfg(any(target_os = "macos", test))]
fn probe_one(proxy: Option<u16>, url: &str, deadline: Instant) -> ProbeOutcome {
    let Some(timeout) = budget(deadline) else {
        return ProbeOutcome::unknown();
    };
    let mut builder = ureq::AgentBuilder::new()
        .try_proxy_from_env(false)
        .redirects(0)
        .timeout(timeout.min(PROBE_TIMEOUT));
    if let Some(port) = proxy {
        let Ok(proxy) = ureq::Proxy::new(format!("http://127.0.0.1:{port}")) else {
            return ProbeOutcome::unknown();
        };
        builder = builder.proxy(proxy);
    }
    request_outcome(builder.build().head(url), 204)
}

#[cfg(any(target_os = "macos", test))]
fn request_outcome(request: ureq::Request, expected_status: u16) -> ProbeOutcome {
    match request.call() {
        Ok(resp) => ProbeOutcome {
            class: classify_status(resp.status(), expected_status).into(),
            http_status: Some(resp.status()),
        },
        Err(ureq::Error::Status(code, _)) => ProbeOutcome {
            class: classify_status(code, expected_status).into(),
            http_status: Some(code),
        },
        Err(error) => ProbeOutcome {
            class: classify_transport(&error.to_string()).into(),
            http_status: None,
        },
    }
}

#[cfg(any(target_os = "macos", test))]
fn probe_with_review(proxy: Option<u16>, urls: &[&str; 2], deadline: Instant) -> ProbeEvidence {
    let primary = probe_one(proxy, urls[0], deadline);
    let review = (!primary.is_ok()).then(|| probe_one(proxy, urls[1], deadline));
    ProbeEvidence { primary, review }
}

#[cfg(target_os = "macos")]
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// 用临时文件接收输出，避免子进程管道填满；超时 kill + wait，不遗留进程。
#[cfg(target_os = "macos")]
fn network_context(deadline: Instant) -> Option<String> {
    let output = tempfile::tempfile().ok()?;
    let mut reader = output.try_clone().ok()?;
    let mut child = crate::process::command("/usr/sbin/scutil")
        .arg("--nwi")
        .stdout(output)
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let local_deadline = deadline.min(Instant::now() + Duration::from_secs(1));
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Err(_) => break false,
            _ if Instant::now() >= local_deadline => break false,
            _ => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    if !success {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }
    use std::io::{Seek, SeekFrom};
    reader.seek(SeekFrom::Start(0)).ok()?;
    let mut bytes = Vec::new();
    reader.take(MAX_RECORD_BYTES).read_to_end(&mut bytes).ok()?;
    Some(format!("{:x}", Sha256::digest(bytes)))
}

#[cfg(target_os = "macos")]
fn generation(
    state: &AppState,
    revision: u64,
    network_context: Option<String>,
) -> Option<Generation> {
    if state.exit.ensure_open().is_err()
        || state.config_transition.load(Ordering::SeqCst)
        || !state.sysproxy.observation_ready()
    {
        return None;
    }
    let core = state.core();
    if core.status() != crate::engine::manager::CoreStatus::Running {
        return None;
    }
    let proxy = state.sysproxy.status();
    let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
    let data = store.data();
    let profile = data
        .active_profile
        .as_deref()
        .and_then(|id| data.profile(id));
    let encoded = serde_json::to_vec(&(&data.engine, profile, &data.overrides)).ok()?;
    Some(Generation {
        session: SESSION.clone(),
        revision,
        profile_id: data.active_profile.clone(),
        mixed_port: data.engine.mixed_port,
        core_pid: core.pid(),
        tun_enabled: data.engine.tun_enable,
        sysproxy_enabled: proxy.enabled,
        config_digest: format!("{:x}", Sha256::digest(encoded)),
        network_context,
    })
}

#[cfg(target_os = "macos")]
fn snapshot(state: &AppState, deadline: Instant) -> Option<Generation> {
    let network = network_context(deadline);
    let revision = *REVISION.lock().unwrap_or_else(|e| e.into_inner());
    generation(state, revision, network)
}

// 网络恢复复用同一代标识与 HTTPS 探针；等待开始前取样，执行前在 core_op 下复核。
#[cfg(target_os = "macos")]
pub(crate) fn recovery_snapshot(state: &AppState) -> Option<Generation> {
    let gen = snapshot(state, Instant::now() + Duration::from_secs(1))?;
    (active(&gen) && gen.network_context.is_some()).then_some(gen)
}

#[cfg(target_os = "macos")]
pub(crate) fn recovery_current(state: &AppState, gen: &Generation) -> bool {
    recovery_snapshot(state).as_ref() == Some(gen)
}

#[cfg(target_os = "macos")]
pub(crate) fn direct_ready(deadline: Instant) -> bool {
    probe_with_review(None, &DIRECT_CN, deadline).reachable()
}

/// 与周期观察共享单飞及发布规则。只有真正成功的代理 HTTPS 才能解除故障。
#[cfg(target_os = "macos")]
pub(crate) fn inspect_network_change(
    handle: &tauri::AppHandle,
    state: &AppState,
    gen: &Generation,
) -> Option<FaultEvidence> {
    let _flight = Flight::enter(&ROUND_IN_FLIGHT)?;
    if !recovery_current(state, gen) {
        return None;
    }
    let deadline = Instant::now() + ROUND_TIMEOUT;
    let started = now_ms();
    let probe = probe_with_review(Some(gen.mixed_port), &PROXY_TARGETS, deadline);
    let observed_at = now_ms();
    let mut existing = read_record(&state.data_dir.join("sysproxy-fault.json"));
    if existing.as_ref().is_some_and(|e| e.generation != *gen) {
        publish(handle, state, gen, None);
        existing = None;
    }
    let mut evidence = run_round(handle, state, gen.clone(), probe.clone(), started, deadline);
    evidence.resolved_at_ms = probe.reachable().then_some(observed_at);
    match decide(existing.as_ref(), gen, &probe) {
        Action::Capture => publish(handle, state, gen, Some(&evidence)),
        Action::Resolve => {
            if let Some(mut old) = existing {
                old.resolved_at_ms = Some(observed_at);
                old.proxy_https = probe;
                publish(handle, state, gen, Some(&old));
            }
        }
        Action::Nothing => {}
    }
    Some(evidence)
}

#[cfg(any(target_os = "macos", test))]
fn active(gen: &Generation) -> bool {
    !gen.tun_enabled && gen.sysproxy_enabled
}

#[cfg(any(target_os = "macos", test))]
fn read_record(path: &Path) -> Option<FaultEvidence> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

#[cfg(target_os = "macos")]
pub fn load(state: &AppState) -> Option<FaultEvidence> {
    let evidence = read_record(&state.data_dir.join("sysproxy-fault.json"))?;
    if evidence.resolved_at_ms.is_none() {
        let current = snapshot(state, Instant::now() + Duration::from_secs(1))?;
        if !active(&current) || evidence.generation != current {
            return None;
        }
    }
    Some(evidence)
}

// 第一版只在 macOS 采集。其他平台保留 IPC 合约，不读取或生成证据。
#[cfg(not(target_os = "macos"))]
pub fn load(_: &AppState) -> Option<serde_json::Value> {
    None
}

#[cfg(any(target_os = "macos", test))]
fn save(path: &Path, evidence: &FaultEvidence) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(evidence).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err("fault evidence exceeds 16KB".into());
    }
    let parent = path
        .parent()
        .ok_or("fault evidence has no parent directory")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn publish(
    handle: &tauri::AppHandle,
    state: &AppState,
    gen: &Generation,
    evidence: Option<&FaultEvidence>,
) {
    // core_op 与公共配置事务串行，REVISION 与接管入口串行，检查和写入不可分开。
    let Ok(_core) = state.core_op.try_lock() else {
        return;
    };
    let revision = REVISION.lock().unwrap_or_else(|e| e.into_inner());
    let current = generation(
        state,
        *revision,
        network_context(Instant::now() + Duration::from_secs(1)),
    );
    if current.as_ref() != Some(gen) {
        return;
    }
    let path = state.data_dir.join("sysproxy-fault.json");
    let result = match evidence {
        Some(evidence) => save(&path, evidence),
        None => match std::fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.to_string()),
            _ => Ok(()),
        },
    };
    match result {
        Ok(()) => {
            let _ = handle.emit("sysproxy://fault-updated", ());
        }
        Err(_) => eprintln!("could not persist system proxy fault evidence"),
    }
}

#[cfg(target_os = "macos")]
fn observe_endpoints(handle: &tauri::AppHandle, deadline: Instant) -> Vec<String> {
    let Some(flight) = Flight::enter(&OBSERVATION_IN_FLIGHT) else {
        return vec![];
    };
    let (send, recv) = std::sync::mpsc::sync_channel(1);
    let handle = handle.clone();
    std::thread::spawn(move || {
        let _flight = flight;
        let Some(state) = handle.try_state::<AppState>() else {
            return;
        };
        let observations: Vec<_> = state
            .sysproxy
            .external_endpoints()
            .into_iter()
            .take(12)
            .map(|(service, endpoint)| {
                format!(
                    "{} → {}:{}",
                    service.chars().take(40).collect::<String>(),
                    if endpoint
                        .host
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || ".:-_[]".contains(c))
                    {
                        endpoint.host.chars().take(80).collect::<String>()
                    } else {
                        "unknown".into()
                    },
                    endpoint.port
                )
            })
            .collect();
        let _ = send.send(observations);
    });
    budget(deadline)
        .and_then(|timeout| recv.recv_timeout(timeout.min(PROBE_TIMEOUT)).ok())
        .unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn run_round(
    handle: &tauri::AppHandle,
    state: &AppState,
    gen: Generation,
    proxy_https: ProbeEvidence,
    started_at_ms: u64,
    deadline: Instant,
) -> FaultEvidence {
    let sysproxy_observed = observe_endpoints(handle, deadline);
    let mixed_port_ok = budget(deadline).is_some_and(|timeout| {
        TcpStream::connect_timeout(
            &([127, 0, 0, 1], gen.mixed_port).into(),
            timeout.min(PROBE_TIMEOUT),
        )
        .is_ok()
    });
    let controller = state
        .core()
        .controller()
        .and_then(|(port, secret)| {
            let timeout = budget(deadline)?;
            Some(request_outcome(
                ureq::AgentBuilder::new()
                    .try_proxy_from_env(false)
                    .timeout(timeout.min(PROBE_TIMEOUT))
                    .build()
                    .get(&format!("http://127.0.0.1:{port}/version"))
                    .set("Authorization", &format!("Bearer {secret}")),
                200,
            ))
        })
        .unwrap_or_else(ProbeOutcome::unknown);
    let direct_cn = probe_with_review(None, &DIRECT_CN, deadline);
    let direct_global = probe_with_review(None, &DIRECT_GLOBAL, deadline);
    FaultEvidence {
        mode: "sysproxy".into(),
        started_at_ms,
        resolved_at_ms: proxy_https.reachable().then(now_ms),
        profile_id: gen.profile_id.clone(),
        mixed_port: gen.mixed_port,
        generation: gen,
        sysproxy_observed,
        mixed_port_ok,
        controller,
        direct_path: "unverified".into(),
        direct_cn,
        direct_global,
        proxy_https,
    }
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, PartialEq, Eq)]
enum Action {
    Nothing,
    Resolve,
    Capture,
}
#[cfg(any(target_os = "macos", test))]
fn decide(existing: Option<&FaultEvidence>, gen: &Generation, probe: &ProbeEvidence) -> Action {
    let same = existing.filter(|e| &e.generation == gen);
    if probe.reachable() {
        if same.is_some_and(|e| e.resolved_at_ms.is_none()) {
            Action::Resolve
        } else if same.is_none() && !probe.primary.is_ok() {
            Action::Capture
        } else {
            Action::Nothing
        }
    } else if same.is_some_and(|e| e.resolved_at_ms.is_none()) {
        Action::Nothing
    } else {
        Action::Capture
    }
}

#[cfg(target_os = "macos")]
pub fn tick(handle: &tauri::AppHandle, ticks: u32) {
    if !ticks.is_multiple_of(CANARY_INTERVAL_TICKS) {
        return;
    }
    let Some(flight) = Flight::enter(&ROUND_IN_FLIGHT) else {
        return;
    };
    let handle = handle.clone();
    std::thread::spawn(move || {
        let _flight = flight;
        let Some(state) = handle.try_state::<AppState>() else {
            return;
        };
        let deadline = Instant::now() + ROUND_TIMEOUT;
        let Some(gen) = snapshot(&state, deadline) else {
            return;
        };
        let mut existing = read_record(&state.data_dir.join("sysproxy-fault.json"));
        if existing
            .as_ref()
            .is_some_and(|e| e.resolved_at_ms.is_none() && e.generation != gen)
        {
            publish(&handle, &state, &gen, None);
            existing = None;
        }
        if !active(&gen) {
            if existing
                .as_ref()
                .is_some_and(|e| e.resolved_at_ms.is_none())
            {
                publish(&handle, &state, &gen, None);
            }
            return;
        }
        let started = now_ms();
        let probe = probe_with_review(Some(gen.mixed_port), &PROXY_TARGETS, deadline);
        match decide(existing.as_ref(), &gen, &probe) {
            Action::Nothing => {}
            Action::Resolve => {
                if let Some(mut evidence) = existing {
                    evidence.resolved_at_ms = Some(now_ms());
                    evidence.proxy_https = probe;
                    publish(&handle, &state, &gen, Some(&evidence));
                }
            }
            Action::Capture => {
                let evidence = run_round(&handle, &state, gen.clone(), probe, started, deadline);
                publish(&handle, &state, &gen, Some(&evidence));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn outcome(class: &str, http_status: Option<u16>) -> ProbeOutcome {
        ProbeOutcome {
            class: class.into(),
            http_status,
        }
    }
    fn healthy() -> ProbeEvidence {
        ProbeEvidence {
            primary: outcome("ok", Some(204)),
            review: None,
        }
    }
    fn failed() -> ProbeEvidence {
        ProbeEvidence {
            primary: outcome("timeout", None),
            review: Some(outcome("timeout", None)),
        }
    }
    fn gen() -> Generation {
        Generation {
            session: "test-session".into(),
            revision: 1,
            profile_id: Some("A".into()),
            mixed_port: 7898,
            core_pid: Some(1),
            tun_enabled: false,
            sysproxy_enabled: true,
            config_digest: "config-a".into(),
            network_context: Some("network-a".into()),
        }
    }
    fn evidence() -> FaultEvidence {
        FaultEvidence {
            mode: "sysproxy".into(),
            started_at_ms: 1,
            resolved_at_ms: None,
            generation: gen(),
            profile_id: Some("A".into()),
            mixed_port: 7898,
            sysproxy_observed: vec![],
            mixed_port_ok: true,
            controller: outcome("ok", Some(200)),
            direct_path: "unverified".into(),
            direct_cn: healthy(),
            direct_global: healthy(),
            proxy_https: failed(),
        }
    }
    fn server(statuses: Vec<u16>) -> (u16, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let worker = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut requests = Vec::new();
            for status in statuses {
                let mut conn = loop {
                    match listener.accept() {
                        Ok((conn, _)) => break conn,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline, "missing probe request");
                            std::thread::sleep(Duration::from_millis(2));
                        }
                        Err(error) => panic!("accept: {error}"),
                    }
                };
                conn.set_nonblocking(false).unwrap();
                conn.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
                let mut request = Vec::new();
                let mut byte = [0u8];
                while !request.ends_with(b"\r\n\r\n") {
                    if conn.read(&mut byte).unwrap() == 0 {
                        break;
                    }
                    request.push(byte[0]);
                }
                requests.push(
                    String::from_utf8(request)
                        .unwrap()
                        .lines()
                        .next()
                        .unwrap()
                        .to_owned(),
                );
                write!(
                    conn,
                    "HTTP/1.1 {status} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
            }
            requests
        });
        (port, worker)
    }

    #[test]
    fn https_probe_exposes_connect_failure_even_when_http_works() {
        assert!(PROXY_TARGETS.iter().all(|url| url.starts_with("https://")));
        let (port, worker) = server(vec![204, 502, 502]);
        let deadline = Instant::now() + Duration::from_secs(4);
        assert!(probe_one(Some(port), "http://example.invalid/generate_204", deadline).is_ok());
        let probe = probe_with_review(Some(port), &PROXY_TARGETS, deadline);
        assert!(!probe.reachable());
        assert_eq!(decide(None, &gen(), &probe), Action::Capture);
        let requests = worker.join().unwrap();
        assert!(requests[0].starts_with("HEAD http://"));
        assert!(requests[1..]
            .iter()
            .all(|line| line.starts_with("CONNECT ")));
    }

    #[test]
    fn site_limits_survive_successful_review_and_use_the_same_recovery_rule() {
        for status in [403, 429] {
            let (port, worker) = server(vec![status, 204]);
            let first = format!("http://127.0.0.1:{port}/first");
            let second = format!("http://127.0.0.1:{port}/second");
            let probe = probe_with_review(
                None,
                &[&first, &second],
                Instant::now() + Duration::from_secs(3),
            );
            assert_eq!(probe.primary, outcome("http_status", Some(status)));
            assert_eq!(probe.review, Some(outcome("ok", Some(204))));
            assert!(probe.reachable());
            assert_eq!(decide(None, &gen(), &probe), Action::Capture);
            let mut old = evidence();
            assert_eq!(decide(Some(&old), &gen(), &probe), Action::Resolve);
            old.resolved_at_ms = Some(2);
            assert_eq!(decide(Some(&old), &gen(), &probe), Action::Nothing);
            worker.join().unwrap();
        }
    }

    #[test]
    fn captive_portal_and_http_errors_are_not_readiness_or_recovery() {
        for status in [200, 302, 403, 429, 503] {
            let (port, worker) = server(vec![status, status]);
            let url = format!("http://127.0.0.1:{port}/generate_204");
            let probe =
                probe_with_review(None, &[&url, &url], Instant::now() + Duration::from_secs(3));
            assert!(!probe.reachable());
            assert_eq!(decide(Some(&evidence()), &gen(), &probe), Action::Nothing);
            assert_eq!(worker.join().unwrap().len(), 2);
        }
    }

    #[test]
    fn old_generation_cannot_block_new_faults_or_be_resolved_by_them() {
        let old = evidence();
        let original = gen();
        let mut variants = Vec::new();
        let mut next = original.clone();
        next.profile_id = Some("B".into());
        variants.push(next);
        let mut next = original.clone();
        next.core_pid = Some(2);
        variants.push(next);
        let mut next = original.clone();
        next.config_digest = "updated-same-profile".into();
        variants.push(next);
        let mut next = original.clone();
        next.network_context = Some("network-b".into());
        variants.push(next);
        let mut next = original.clone();
        next.revision += 2;
        variants.push(next); // off -> on / A -> B -> A
        let mut next = original.clone();
        next.session = "new-app-session".into();
        variants.push(next);
        for new in variants {
            assert_eq!(decide(Some(&old), &new, &failed()), Action::Capture);
            assert_eq!(decide(Some(&old), &new, &healthy()), Action::Nothing);
            assert_ne!(old.generation, new); // 实际发布前也使用完整比较
        }
        let mut disabled = original;
        disabled.sysproxy_enabled = false;
        assert!(!active(&disabled));
        assert_ne!(old.generation, disabled);
    }

    #[test]
    fn same_fault_is_deduplicated_and_recovery_stays_available() {
        let old = evidence();
        assert_eq!(decide(Some(&old), &gen(), &failed()), Action::Nothing);
        assert_eq!(decide(Some(&old), &gen(), &healthy()), Action::Resolve);
        let flag = AtomicBool::new(false);
        let flight = Flight::enter(&flag).unwrap();
        assert!(Flight::enter(&flag).is_none());
        drop(flight);
        assert!(Flight::enter(&flag).is_some());
    }

    #[test]
    fn evidence_is_atomic_private_and_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("fault.json");
        let old = evidence();
        save(&path, &old).unwrap();
        assert_eq!(read_record(&path), Some(old.clone()));
        let mut too_large = old.clone();
        too_large.sysproxy_observed = vec!["x".repeat(MAX_RECORD_BYTES as usize)];
        assert!(save(&path, &too_large).is_err());
        assert_eq!(read_record(&path), Some(old));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::write(&path, vec![b'x'; MAX_RECORD_BYTES as usize + 1]).unwrap();
        assert!(read_record(&path).is_none());
        std::fs::write(&path, br#"{"mode":"sysproxy","startedAtMs":1}"#).unwrap();
        assert!(read_record(&path).is_none()); // 旧格式没有完整代，不能继续抑制取证
    }

    #[test]
    fn deadline_prevents_extra_network_work_and_redirects_are_not_recovery() {
        assert!(budget(Instant::now() - Duration::from_secs(1)).is_none());
        let probe = probe_with_review(
            Some(1),
            &PROXY_TARGETS,
            Instant::now() - Duration::from_secs(1),
        );
        assert!(!probe.reachable());
        for status in [200, 302, 403, 429, 502] {
            assert_eq!(classify_status(status, 204), "http_status");
        }
        assert_eq!(classify_status(204, 204), "ok");
        assert_eq!(
            classify_transport("Connection failed: invalid peer certificate"),
            "tls"
        );
        assert_eq!(classify_transport("Network timeout"), "timeout");
        assert_eq!(classify_transport("Dns failed"), "dns");
    }

    #[test]
    fn controller_200_is_healthy_but_not_a_successful_204_canary() {
        let (port, worker) = server(vec![200, 200]);
        let url = format!("http://127.0.0.1:{port}/version");
        let agent = ureq::AgentBuilder::new()
            .try_proxy_from_env(false)
            .timeout(Duration::from_secs(1))
            .build();
        assert!(request_outcome(agent.get(&url), 200).is_ok());
        assert!(!probe_one(None, &url, Instant::now() + Duration::from_secs(1)).is_ok());
        worker.join().unwrap();
    }
}
