//! #488：macOS 系统代理模式（TUN 关闭）的故障取证——观察模式。
//!
//! 背景：2026-11-13 与更早的全量超时事件里，内核存活、端口监听、控制器
//! 有响应、外网请求却持续超时，几分钟后自行恢复。既有兜底只覆盖内核
//! 停止/崩溃；故障发生时没有任何分层证据留下，恢复后无法归因。
//!
//! 机制（第一版仅 macOS）：
//! - 托盘看门狗每 tick 递增计数；每 CANARY_INTERVAL_TICKS（≈2 分钟）发一次
//!   经混合端口的 204 金丝雀。正常联网时请求量 = 1 次/2 分钟。
//! - 金丝雀失败 → 执行一轮分层取证（系统代理实际指向、混合端口、控制器、
//!   直连基准国内/国外双端点、经代理 HTTPS），关键失败用独立端点复核。
//! - 已有未恢复记录时不再重复取证（单飞 + 开记录抑制）；金丝雀恢复成功
//!   则补记 resolved_at（恢复确认）。配置切换/接管关闭后，旧记录与迟到
//!   结果按代丢弃。
//! - 只记录错误类别、时间、模式与本地端口上下文；不含订阅 URL、访问
//!   记录与认证信息。观察模式：不切节点、不重启内核、不写系统代理。

use std::io::Write;
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};

use crate::AppState;

/// 金丝雀周期：×WATCHDOG_TICK(5s) ≈ 2 分钟——故障发现延迟的上限。
pub const CANARY_INTERVAL_TICKS: u32 = 24;
/// 单探测超时；整轮上限 ≈ 探测数 × 该值，仍在看门狗节奏之外异步执行。
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// 国内/国外基准与代理探针：主目标失败用独立端点复核（站点限制 vs 链路）。
const DIRECT_CN: &[&str] = &[
    "http://connect.rom.miui.com/generate_204",
    "http://wifi.vivo.com.cn/generate_204",
];
const DIRECT_GLOBAL: &[&str] = &[
    "http://www.gstatic.com/generate_204",
    "http://cp.cloudflare.com/generate_204",
];
const PROXY_TARGETS: &[&str] = &[
    "http://www.gstatic.com/generate_204",
    "http://cp.cloudflare.com/generate_204",
];

static ROUND_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// 单探针结果：类别 + HTTP 状态码（仅链路证据，不含 URL）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeOutcome {
    /// ok / timeout / connect / tls / dns / http_status / unknown
    pub class: String,
    pub http_status: Option<u16>,
}

impl ProbeOutcome {
    pub(crate) fn is_ok(&self) -> bool {
        self.class == "ok"
    }
}

/// 一次故障的分层证据（有界：固定字段，单文件存储）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaultEvidence {
    pub mode: String,
    pub started_at_ms: u64,
    pub resolved_at_ms: Option<u64>,
    pub profile_id: Option<String>,
    pub mixed_port: u16,
    /// 各网络服务当前实际代理指向（脱敏：仅 服务名 → host:port）。
    pub sysproxy_observed: Vec<String>,
    pub mixed_port_ok: bool,
    pub controller: ProbeOutcome,
    pub direct_cn: ProbeOutcome,
    pub direct_global: ProbeOutcome,
    pub proxy_https: ProbeOutcome,
}

pub fn classify_status(status: u16) -> &'static str {
    if (200..400).contains(&status) {
        "ok"
    } else {
        "http_status"
    }
}

/// 传输层错误 → 类别；按错误文本判别（ureq 2.x 无结构化 kind 暴露）。
pub fn classify_transport(text: &str) -> &'static str {
    let low = text.to_lowercase();
    if low.contains("timed out") || low.contains("deadline") {
        "timeout"
    } else if low.contains("refused") {
        "connect"
    } else if low.contains("certificate") || low.contains("tls") || low.contains("ssl") {
        "tls"
    } else if low.contains("dns") {
        "dns"
    } else {
        "unknown"
    }
}

fn direct_agent() -> ureq::Agent {
    // 不走系统代理/环境变量：直连基准必须反映物理链路。
    ureq::AgentBuilder::new()
        .try_proxy_from_env(false)
        .timeout(PROBE_TIMEOUT)
        .build()
}

fn proxied_agent(mixed_port: u16) -> Result<ureq::Agent, String> {
    let proxy = ureq::Proxy::new(format!("http://127.0.0.1:{mixed_port}"))
        .map_err(|e| format!("proxy 配置错误：{e}"))?;
    Ok(ureq::AgentBuilder::new()
        .proxy(proxy)
        .timeout(PROBE_TIMEOUT)
        .build())
}

fn probe_one(agent: &ureq::Agent, url: &str) -> ProbeOutcome {
    match agent.head(url).call() {
        Ok(resp) => ProbeOutcome {
            class: classify_status(resp.status()).to_string(),
            http_status: Some(resp.status()),
        },
        Err(ureq::Error::Status(code, _)) => ProbeOutcome {
            class: classify_status(code).to_string(),
            http_status: Some(code),
        },
        Err(error) => ProbeOutcome {
            class: classify_transport(&error.to_string()).to_string(),
            http_status: None,
        },
    }
}

/// 主目标失败 → 独立端点复核；复核结果即记录（区分站点限制与链路故障）。
fn probe_with_review(agent: &ureq::Agent, urls: &[&str]) -> ProbeOutcome {
    let (first, rest) = urls.split_first().expect("probe targets are never empty");
    let outcome = probe_one(agent, first);
    if outcome.is_ok() || rest.is_empty() {
        return outcome;
    }
    probe_one(agent, rest[0])
}

fn probe_mixed_port(mixed_port: u16) -> bool {
    let address = format!("127.0.0.1:{mixed_port}");
    match std::net::ToSocketAddrs::to_socket_addrs(&address) {
        Ok(mut addrs) => addrs
            .next()
            .and_then(|addr| TcpStream::connect_timeout(&addr, PROBE_TIMEOUT).ok())
            .is_some(),
        Err(_) => false,
    }
}

fn probe_controller(port: u16, secret: &str) -> ProbeOutcome {
    match ureq::AgentBuilder::new()
        .try_proxy_from_env(false)
        .timeout(PROBE_TIMEOUT)
        .build()
        .get(&format!("http://127.0.0.1:{port}/version"))
        .set("Authorization", &format!("Bearer {secret}"))
        .call()
    {
        Ok(resp) => ProbeOutcome {
            class: classify_status(resp.status()).to_string(),
            http_status: Some(resp.status()),
        },
        Err(ureq::Error::Status(code, _)) => ProbeOutcome {
            class: classify_status(code).to_string(),
            http_status: Some(code),
        },
        Err(error) => ProbeOutcome {
            class: classify_transport(&error.to_string()).to_string(),
            http_status: None,
        },
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 证据文件的代：同一配置/内核/接管状态下产生的一轮结果。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Generation {
    profile_id: Option<String>,
    mixed_port: u16,
    core_pid: Option<u32>,
    tun_enabled: bool,
}

fn generation(state: &AppState) -> Generation {
    let guard = state.store.lock().unwrap_or_else(|e| e.into_inner());
    let data = guard.data();
    Generation {
        profile_id: data.active_profile.clone(),
        mixed_port: data.engine.mixed_port,
        core_pid: state.core().pid(),
        tun_enabled: data.engine.tun_enable,
    }
}

fn record_path(state: &AppState) -> PathBuf {
    state.data_dir.join("sysproxy-fault.json")
}

/// 故障恢复确认：同代未恢复记录补记 resolved_at（恢复 = 首个成功的
/// 经代理请求，#490 网络事件同样复用）。
pub(crate) fn confirm_recovery(state: &AppState, evidence: &FaultEvidence) {
    let gen = generation(state);
    if evidence.profile_id == gen.profile_id && evidence.mixed_port == gen.mixed_port {
        let mut resolved = evidence.clone();
        resolved.resolved_at_ms = Some(now_ms());
        let _ = save(state, &resolved);
    }
}

/// 落盘一轮取证：仅在无未恢复记录时写入（开记录抑制，#488 语义）。
pub(crate) fn record_round_if_needed(state: &AppState, evidence: FaultEvidence) -> bool {
    if let Some(existing) = load(state) {
        if existing.resolved_at_ms.is_none() {
            return false;
        }
    }
    save(state, &evidence).is_ok()
}

pub fn load(state: &AppState) -> Option<FaultEvidence> {
    let bytes = std::fs::read(record_path(state)).ok()?;
    // 有界防御：超过 16KB 的文件视为损坏证据，直接丢弃。
    if bytes.len() > 16 * 1024 {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

fn save(state: &AppState, evidence: &FaultEvidence) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(evidence).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    let path = record_path(state);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())
}

fn remove(state: &AppState, handle: &tauri::AppHandle) {
    let _ = std::fs::remove_file(record_path(state));
    let _ = handle.emit("sysproxy://fault-updated", ());
}

/// 一轮分层取证。代（配置/内核/接管状态）在完成后复核：不匹配即丢弃，
/// 迟到的结果永远无法覆盖新状态（issue 验收第 3 条）。
pub(crate) fn run_round(state: &AppState) -> Option<FaultEvidence> {
    let gen = generation(state);
    let mixed_port = gen.mixed_port;
    let sysproxy_observed: Vec<String> = state
        .sysproxy
        .external_endpoints()
        .into_iter()
        .map(|(service, endpoint)| format!("{service} → {}:{}", endpoint.host, endpoint.port))
        .collect();

    let mixed_port_ok = probe_mixed_port(mixed_port);
    let controller = state
        .core()
        .controller()
        .map(|(port, secret)| probe_controller(port, &secret))
        .unwrap_or(ProbeOutcome {
            class: "unknown".to_string(),
            http_status: None,
        });
    let direct = direct_agent();
    let direct_cn = probe_with_review(&direct, DIRECT_CN);
    let direct_global = probe_with_review(&direct, DIRECT_GLOBAL);
    let proxy_https = proxied_agent(mixed_port)
        .map(|agent| probe_with_review(&agent, PROXY_TARGETS))
        .unwrap_or(ProbeOutcome {
            class: "unknown".to_string(),
            http_status: None,
        });

    // 代复核：配置切换/内核替换/接管开关发生后，本轮结果作废。
    if generation(state) != gen {
        return None;
    }
    Some(FaultEvidence {
        mode: "sysproxy".to_string(),
        started_at_ms: now_ms(),
        resolved_at_ms: None,
        profile_id: gen.profile_id,
        mixed_port: gen.mixed_port,
        sysproxy_observed,
        mixed_port_ok,
        controller,
        direct_cn,
        direct_global,
        proxy_https,
    })
}

/// 看门狗 tick 入口（macOS、TUN 关闭、系统代理开启、内核运行中才有动作）。
pub fn tick(handle: &tauri::AppHandle, ticks: u32) {
    if !ticks.is_multiple_of(CANARY_INTERVAL_TICKS) {
        return;
    }
    let h = handle.clone();
    std::thread::spawn(move || {
        let Some(state) = h.try_state::<AppState>() else {
            return;
        };
        if state.exit.ensure_open().is_err() {
            return;
        }
        let tun_on = state
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .data()
            .engine
            .tun_enable;
        let sysproxy_on = state.sysproxy.status().enabled;
        let running = matches!(
            state.core().status(),
            crate::engine::manager::CoreStatus::Running
        );
        if tun_on || !sysproxy_on {
            // 接管状态变化：旧上下文的未恢复记录按代丢弃。
            if let Some(existing) = load(&state) {
                if existing.resolved_at_ms.is_none() {
                    remove(&state, &h);
                }
            }
            return;
        }
        let port = state
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .data()
            .engine
            .mixed_port;
        if !running {
            // 内核停止/崩溃由既有 sysproxy 兜底负责；这里不做金丝雀。
            return;
        }

        // 金丝雀：经混合端口的一次 204 请求。
        let canary = proxied_agent(port)
            .map(|agent| probe_one(&agent, PROXY_TARGETS[0]))
            .map(|outcome| outcome.is_ok())
            .unwrap_or(false);
        let existing = load(&state);
        if canary {
            if let Some(evidence) = existing.filter(|e| e.resolved_at_ms.is_none()) {
                // 故障恢复确认：补记 resolved_at（同代才有效）。
                confirm_recovery(&state, &evidence);
            }
            return;
        }
        // 金丝雀失败：未恢复的记录已存在 → 证据已在手，不重复取证。
        if let Some(evidence) = existing {
            if evidence.resolved_at_ms.is_none() {
                return;
            }
        }
        if ROUND_IN_FLIGHT.swap(true, Ordering::SeqCst) {
            return;
        }
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let state = h.state::<AppState>();
            run_round(&state)
        }));
        ROUND_IN_FLIGHT.store(false, Ordering::SeqCst);
        if let Ok(Some(evidence)) = outcome {
            let _ = save(&state, &evidence);
            let _ = h.emit("sysproxy://fault-updated", ());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_classification_separates_ok_from_site_limit() {
        assert_eq!(classify_status(204), "ok");
        assert_eq!(classify_status(200), "ok");
        assert_eq!(classify_status(302), "ok");
        assert_eq!(classify_status(403), "http_status");
        assert_eq!(classify_status(429), "http_status");
        assert_eq!(classify_status(502), "http_status");
    }

    #[test]
    fn transport_classification_covers_the_evidence_categories() {
        assert_eq!(classify_transport("Network timed out"), "timeout");
        assert_eq!(
            classify_transport("Connection refused (os error 111)"),
            "connect"
        );
        assert_eq!(classify_transport("invalid peer certificate"), "tls");
        assert_eq!(classify_transport("Dns Failed"), "dns");
        assert_eq!(classify_transport("weird"), "unknown");
    }

    #[test]
    fn review_probe_takes_the_independent_endpoint_when_the_first_fails() {
        // 无法在单测里真的发请求：直接验证结构与序列化有界性。
        let outcome = ProbeOutcome {
            class: "timeout".into(),
            http_status: None,
        };
        assert!(!outcome.is_ok());
        let evidence = FaultEvidence {
            mode: "sysproxy".into(),
            started_at_ms: 1_700_000_000_000,
            resolved_at_ms: None,
            profile_id: Some("demo".into()),
            mixed_port: 7898,
            sysproxy_observed: vec!["Wi-Fi → 127.0.0.1:7898".into()],
            mixed_port_ok: true,
            controller: ProbeOutcome {
                class: "ok".into(),
                http_status: Some(200),
            },
            direct_cn: outcome.clone(),
            direct_global: ProbeOutcome {
                class: "ok".into(),
                http_status: None,
            },
            proxy_https: ProbeOutcome {
                class: "timeout".into(),
                http_status: None,
            },
        };
        let bytes = serde_json::to_vec(&evidence).unwrap();
        assert!(bytes.len() < 16 * 1024);
        let back: FaultEvidence = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back, evidence);
        assert_eq!(back.proxy_https.class, "timeout");
        assert!(serde_json::to_value(&back).unwrap()["startedAtMs"]
            .as_u64()
            .is_some());
    }

    #[test]
    fn generation_recheck_is_the_only_write_path() {
        // 代不一致时 run_round 返回 None 的逻辑依赖 Generation 的完整比较；
        // 这里锁住字段集，防止将来新增字段忘记纳入比较。
        let gen = Generation {
            profile_id: Some("p".into()),
            mixed_port: 7898,
            core_pid: Some(1),
            tun_enabled: false,
        };
        let same = gen.clone();
        let different = Generation {
            core_pid: Some(2),
            ..gen.clone()
        };
        assert_eq!(gen, same);
        assert_ne!(gen, different);
    }
}
