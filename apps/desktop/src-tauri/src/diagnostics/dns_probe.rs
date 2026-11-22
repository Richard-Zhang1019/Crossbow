//! #603 内核 DNS 解析诊断：把「系统解析」与「内核 DNS 接口查询（不保证等同逐连接路径）」放进同一份记录。
//!
//! 用户主动触发，不常驻抓取：查询用户指定域名的 A/AAAA，对比系统解析器与
//! 内核 `/dns/query` 的结果、各自耗时与失败类别。两个内核只暴露 RCODE、TTL
//! 和应答地址；上游服务器、缓存命中与内核内部解析耗时拿不到时明确列入
//! `unknown_fields`，不推测。结果只随本次调用返回：不落盘、不发送遥测，
//! 前端的历史仅保留当前会话最近几条并可就地清除。

use std::net::{IpAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
static PROBE: std::sync::Mutex<()> = std::sync::Mutex::new(());
static SYSTEM_ACTIVE: AtomicBool = AtomicBool::new(false);
struct SystemLease;
impl Drop for SystemLease {
    fn drop(&mut self) {
        SYSTEM_ACTIVE.store(false, Ordering::Release);
    }
}
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::AppState;

/// 系统解析器预算：`ToSocketAddrs` 没有超时参数，靠独立线程 + 接收超时兜底。
const SYSTEM_BUDGET: Duration = Duration::from_secs(5);
/// 应答地址上限：只展示前几条，避免把整条 CNAME/A 链塞进界面。
const MAX_ANSWERS: usize = 4;
/// hosts 文件读取上限；超过即视为异常文件，不读、不猜测。
const HOSTS_MAX_BYTES: u64 = 1 << 20;

/// 单条解析路径的结果。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DnsPathResult {
    /// ok / no_record / nxdomain / servfail / refused / timeout / error / unavailable
    pub state: &'static str,
    /// 该路径的往返耗时（系统路径含 OS 解析器，内核路径含控制器 IPC）。
    pub ms: u64,
    /// 内核返回的 RCODE；系统路径拿不到时为 None。
    pub rcode: Option<i64>,
    /// 应答地址（按内核/系统返回顺序，最多 MAX_ANSWERS 条）。
    pub answers: Vec<String>,
    /// 应答中的最小 TTL；系统路径不提供。
    pub ttl: Option<u32>,
    /// 简短原因（已脱敏），不做推测性归因。
    pub note: Option<String>,
}

/// 一次诊断的完整记录。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DnsProbeResult {
    /// 归一化后的查询域名（用户输入，已校验为 ASCII 主机名）。
    pub profile_id: Option<String>,
    pub config_revision: u64,
    pub domain: String,
    /// A / AAAA。
    pub record_type: &'static str,
    pub at_ms: u64,
    pub engine: &'static str,
    pub core_running: bool,
    pub core_pid: Option<u32>,
    pub core_version: Option<String>,
    /// inherit / custom：#598 的 DNS 意图，用来解释内核解析来源。
    pub dns_mode: &'static str,
    pub system: DnsPathResult,
    pub kernel: DnsPathResult,
    /// both_ok / system_only / kernel_only / both_fail / kernel_unavailable
    pub verdict: &'static str,
    /// hosts 文件里是否有该主机名；有则系统解析可能绕开 DNS。
    pub hosts_override: bool,
    /// 本接口拿不到的字段，界面必须原样标为「未知」。
    pub unknown_fields: Vec<&'static str>,
}

impl DnsPathResult {
    fn unavailable(note: &str) -> Self {
        Self {
            state: "unavailable",
            ms: 0,
            rcode: None,
            answers: Vec::new(),
            ttl: None,
            note: Some(crate::notifications::redact(note)),
        }
    }

    fn failed(state: &'static str, ms: u64, note: String) -> Self {
        Self {
            state,
            ms,
            rcode: None,
            answers: Vec::new(),
            ttl: None,
            note: Some(crate::notifications::redact(&note)),
        }
    }

    fn ok(&self) -> bool {
        self.state == "ok"
    }
}

/// 校验并归一化域名：只接受 ASCII 主机名（punycode 之外的输入明确拒绝，
/// 不擅自转码），允许单标签内网域，去掉末尾根点。
pub(crate) fn normalize_domain(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("请输入要检查的域名".into());
    }
    let name = trimmed
        .strip_suffix('.')
        .unwrap_or(trimmed)
        .to_ascii_lowercase();
    if name.parse::<IpAddr>().is_ok() {
        return Err("请输入域名，不是 IP 地址 / Enter a domain, not an IP address".into());
    }
    if name.len() > 253 {
        return Err("域名过长（最多 253 个字符）".into());
    }
    if !name.is_ascii() {
        return Err("仅支持 ASCII 主机名；国际化域名请填写 punycode".into());
    }
    if name.contains([' ', '\t', '/', '\\', ':', '@', '#', '?', '%', '*']) {
        return Err("域名包含不支持的字符".into());
    }
    for label in name.split('.') {
        if label.is_empty() {
            return Err("域名存在空标签".into());
        }
        if label.len() > 63 {
            return Err("域名标签过长（每段最多 63 个字符）".into());
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err("域名标签不能以连字符开头或结尾".into());
        }
        if !label
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            return Err("域名含不支持的字符".into());
        }
    }
    Ok(name)
}

/// 只开放 A/AAAA：其他记录类型不在这条诊断的验收范围内。
pub(crate) fn parse_record_type(raw: &str) -> Result<&'static str, String> {
    match raw.trim().to_ascii_uppercase().as_str() {
        "A" => Ok("A"),
        "AAAA" => Ok("AAAA"),
        _ => Err("仅支持 A 或 AAAA 记录".into()),
    }
}

fn rcode_state(status: i64) -> Option<&'static str> {
    match status {
        0 => Some("ok"),
        2 => Some("servfail"),
        3 => Some("nxdomain"),
        5 => Some("refused"),
        _ => None,
    }
}

/// 解析内核 `/dns/query` 的 DNS 报文 JSON（mihomo 与 sing-box 返回同一形态）。
pub(crate) fn parse_kernel_reply(
    reply: &serde_json::Value,
    record_type: &'static str,
    ms: u64,
) -> DnsPathResult {
    let Some(status) = reply["Status"].as_i64() else {
        return DnsPathResult::failed("error", ms, "内核返回了无法识别的 DNS 报文".into());
    };
    let state = match rcode_state(status) {
        Some(state) => state,
        None => {
            return DnsPathResult::failed("error", ms, format!("内核返回 RCODE {status}"));
        }
    };
    let want_type = if record_type == "AAAA" { 28 } else { 1 };
    let mut answers = Vec::new();
    let mut ttl: Option<u32> = None;
    if let Some(list) = reply["Answer"].as_array() {
        for answer in list {
            if answer["type"].as_i64() != Some(want_type) {
                continue;
            }
            if let Some(data) = answer["data"].as_str().filter(|value| {
                value
                    .parse::<IpAddr>()
                    .is_ok_and(|ip| ip.is_ipv6() == (record_type == "AAAA"))
            }) {
                if answers.len() < MAX_ANSWERS && !answers.iter().any(|a| a == data) {
                    answers.push(data.to_string());
                }
            }
            if let Some(value) = answer["TTL"].as_u64() {
                let value = value.min(u32::MAX as u64) as u32;
                ttl = Some(ttl.map_or(value, |current| current.min(value)));
            }
        }
    }
    let state = if state == "ok" && answers.is_empty() {
        "no_record"
    } else {
        state
    };
    DnsPathResult {
        state,
        ms,
        rcode: Some(status),
        answers,
        ttl,
        note: (state == "no_record")
            .then(|| format!("内核返回 RCODE 0，但没有 {record_type} 应答")),
    }
}

fn family_answers(addrs: Vec<String>, record_type: &str) -> Vec<String> {
    let mut answers = Vec::new();
    for value in addrs {
        if let Ok(ip) = value.parse::<IpAddr>() {
            if ip.is_ipv6() == (record_type == "AAAA") && !answers.contains(&ip.to_string()) {
                answers.push(ip.to_string());
                if answers.len() == MAX_ANSWERS {
                    break;
                }
            }
        }
    }
    answers
}
fn system_probe(domain: &str, record_type: &str) -> DnsPathResult {
    let start = Instant::now();
    if SYSTEM_ACTIVE
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        return DnsPathResult::unavailable(
            "此前系统解析仍未返回，请稍后重试 / Previous OS lookup is still running",
        );
    }
    let target = format!("{domain}:0");
    let (tx, rx) = std::sync::mpsc::channel();
    // 解析线程不可取消：超时后丢弃结果，但不会阻塞调用方。
    std::thread::spawn(move || {
        let _lease = SystemLease;
        let resolved = target
            .to_socket_addrs()
            .map(|addrs| addrs.map(|addr| addr.ip().to_string()).collect::<Vec<_>>());
        let _ = tx.send(resolved);
    });
    match rx.recv_timeout(SYSTEM_BUDGET) {
        Ok(Ok(addrs)) => {
            let ms = elapsed_ms(start);
            let answers = family_answers(addrs, record_type);
            if answers.is_empty() {
                DnsPathResult::failed("no_record", ms, "系统解析器没有返回地址".into())
            } else {
                DnsPathResult {
                    state: "ok",
                    ms,
                    rcode: None,
                    answers,
                    ttl: None,
                    note: None,
                }
            }
        }
        Ok(Err(error)) => {
            let ms = elapsed_ms(start);
            DnsPathResult::failed("error", ms, classify_system_error(&error))
        }
        Err(_) => DnsPathResult::failed(
            "timeout",
            elapsed_ms(start),
            "系统解析器在 5 秒内没有返回结果".into(),
        ),
    }
}

fn classify_system_error(error: &std::io::Error) -> String {
    // 系统解析器把 NXDOMAIN、无记录和解析器不可用折叠成同一类错误：
    // 这里只报事实，不猜具体 RCODE。
    let kind = match error.kind() {
        std::io::ErrorKind::NotFound => {
            "系统解析器未返回该主机名的地址（NXDOMAIN 与无记录无法区分）"
        }
        std::io::ErrorKind::TimedOut => "系统解析器超时",
        _ => "系统解析失败",
    };
    format!("{kind}：{error}")
}

fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().min(u64::MAX as u128) as u64
}

fn kernel_probe(
    core: &crate::engine::manager::CoreManager,
    domain: &str,
    record_type: &'static str,
) -> DnsPathResult {
    let Some((port, secret)) = core.controller() else {
        return DnsPathResult::unavailable("内核运行中但没有可用的控制器信息");
    };
    let controller = crate::engine::api::Controller { port, secret };
    let start = Instant::now();
    match controller.dns_query(domain, record_type) {
        Ok(reply) => parse_kernel_reply(&reply, record_type, elapsed_ms(start)),
        Err(error) => DnsPathResult::failed(
            if error.starts_with("DNS query timeout:") {
                "timeout"
            } else {
                "error"
            },
            elapsed_ms(start),
            error,
        ),
    }
}

pub(crate) fn verdict(system: &DnsPathResult, kernel: &DnsPathResult) -> &'static str {
    if kernel.state == "unavailable" {
        return "kernel_unavailable";
    }
    match (system.ok(), kernel.ok()) {
        (true, true) => "both_ok",
        (true, false) => "system_only",
        (false, true) => "kernel_only",
        (false, false) => "both_fail",
    }
}

fn hosts_path() -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        let root = std::env::var_os("SystemRoot")?;
        let path = std::path::PathBuf::from(root)
            .join("System32")
            .join("drivers")
            .join("etc")
            .join("hosts");
        return Some(path);
    }
    #[cfg(not(windows))]
    {
        Some(std::path::PathBuf::from("/etc/hosts"))
    }
}

/// hosts 命中检测：只看静态映射，不解析通配或系统 DNS 缓存。
pub(crate) fn hosts_has(path: &Path, domain: &str) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if meta.len() > HOSTS_MAX_BYTES {
        return false;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    text.lines().any(|line| {
        let entry = line.split('#').next().unwrap_or_default();
        let mut fields = entry.split_whitespace();
        // 首列是地址；其余列是同一地址的多个主机名。
        if !fields.next().is_some_and(|ip| ip.parse::<IpAddr>().is_ok()) {
            return false;
        }
        fields.any(|name| name.eq_ignore_ascii_case(domain))
    })
}

fn context_current(
    before: (u64, Option<u32>, bool),
    after: (u64, Option<u32>, bool),
    transitioning: bool,
) -> bool {
    !transitioning && before == after
}

/// 一次完整诊断。域名/记录类型非法时不发起任何查询。
pub fn run(state: &AppState, domain: &str, record_type: &str) -> Result<DnsProbeResult, String> {
    let _probe = PROBE
        .try_lock()
        .map_err(|_| "DNS 检查仍在进行，请稍后重试 / DNS check is still running".to_string())?;
    let domain = normalize_domain(domain)?;
    let record_type = parse_record_type(record_type)?;
    let config_revision = state.node_delay_revision.load(Ordering::SeqCst);
    if state.config_transition.load(Ordering::SeqCst) {
        return Err("配置正在切换，请稍后重试 / Configuration is changing".into());
    }
    let at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0);
    let (engine, dns_mode, profile_id) = {
        let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        let engine = match store.data().engine.engine {
            crossbow_core::Engine::Mihomo => "mihomo",
            crossbow_core::Engine::SingBox => "sing-box",
        };
        let dns_mode = match store.data().dns.mode {
            crossbow_core::dns_settings::DnsMode::Inherit => "inherit",
            crossbow_core::dns_settings::DnsMode::Custom => "custom",
        };
        (engine, dns_mode, store.data().active_profile.clone())
    };
    let core = state.core();
    let core_running = core.status() == crate::CoreStatus::Running;
    let core_pid = core.pid();
    let core_version = if core_running {
        core.controller().and_then(|(port, secret)| {
            crate::engine::api::Controller { port, secret }
                .version()
                .ok()
                .map(|version| crate::notifications::redact(version.trim()))
        })
    } else {
        None
    };
    let system = system_probe(&domain, record_type);
    let kernel = if core_running {
        kernel_probe(&core, &domain, record_type)
    } else {
        DnsPathResult::unavailable("内核未运行，只记录了系统解析结果")
    };
    if !context_current(
        (config_revision, core_pid, core_running),
        (
            state.node_delay_revision.load(Ordering::SeqCst),
            state.core().pid(),
            state.core().status() == crate::CoreStatus::Running,
        ),
        state.config_transition.load(Ordering::SeqCst),
    ) {
        return Err(
            "配置或内核已变化，本次结果已丢弃 / Configuration or core changed; result discarded"
                .into(),
        );
    }
    let hosts_override = hosts_path().is_some_and(|path| hosts_has(&path, &domain));
    Ok(DnsProbeResult {
        verdict: verdict(&system, &kernel),
        at_ms,
        profile_id,
        config_revision,
        domain,
        record_type,
        engine,
        core_running,
        core_pid,
        core_version,
        dns_mode,
        system,
        kernel,
        hosts_override,
        // 两个内核都没有暴露这些事实：界面必须原样标为未知，不能用 TTL 反推缓存。
        unknown_fields: vec!["upstream", "cache", "kernel_time"],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_validation_rejects_inputs_that_would_change_query_meaning() {
        assert_eq!(
            normalize_domain(" WWW.Gstatic.com. ").unwrap(),
            "www.gstatic.com"
        );
        assert_eq!(normalize_domain("nas").unwrap(), "nas");
        for bad in [
            "",
            "   ",
            "a..b",
            "-a.com",
            "a-.com",
            "a b.com",
            "a/b",
            "a:80",
            "例子.cn",
        ] {
            assert!(normalize_domain(bad).is_err(), "{bad:?} 应被拒绝");
        }
        assert!(normalize_domain(&format!("{}.com", "a".repeat(64))).is_err());
        assert!(
            normalize_domain(&format!("{}.{}", "a".repeat(60), "b".repeat(60).repeat(5))).is_err()
        );
    }

    #[test]
    fn context_rejects_changes_even_if_pid_does_not_change() {
        assert!(context_current(
            (1, Some(10), true),
            (1, Some(10), true),
            false
        ));
        assert!(!context_current(
            (1, Some(10), true),
            (2, Some(10), true),
            false
        ));
        assert!(!context_current(
            (1, Some(10), true),
            (1, Some(11), true),
            false
        ));
        assert!(!context_current(
            (1, Some(10), true),
            (1, Some(10), true),
            true
        ));
    }
    #[test]
    fn system_answers_are_family_specific() {
        let both = vec!["127.0.0.1".into(), "::1".into(), "127.0.0.1".into()];
        assert_eq!(family_answers(both.clone(), "A"), ["127.0.0.1"]);
        assert_eq!(family_answers(both, "AAAA"), ["::1"]);
        assert!(family_answers(vec!["127.0.0.1".into()], "AAAA").is_empty());
    }
    #[test]
    fn only_a_and_aaaa_are_accepted() {
        assert_eq!(parse_record_type("a").unwrap(), "A");
        assert_eq!(parse_record_type(" AAAA ").unwrap(), "AAAA");
        for bad in ["", "MX", "ANY", "1"] {
            assert!(parse_record_type(bad).is_err(), "{bad:?} 应被拒绝");
        }
    }

    #[test]
    fn kernel_reply_keeps_rcode_ttl_and_only_requested_family() {
        let reply = serde_json::json!({
            "Status": 0,
            "Answer": [
                { "name": "a.example.", "type": 5, "TTL": 30, "data": "b.example." },
                { "name": "a.example.", "type": 1, "TTL": 120, "data": "192.0.2.1" },
                { "name": "a.example.", "type": 1, "TTL": 60, "data": "192.0.2.2" },
                { "name": "a.example.", "type": 1, "TTL": 90, "data": "192.0.2.1" },
                { "name": "a.example.", "type": 28, "TTL": 10, "data": "2001:db8::1" }
            ]
        });
        let result = parse_kernel_reply(&reply, "A", 12);
        assert_eq!(result.state, "ok");
        assert_eq!(result.rcode, Some(0));
        assert_eq!(result.ttl, Some(60));
        assert_eq!(result.answers, vec!["192.0.2.1", "192.0.2.2"]);
        assert_eq!(result.ms, 12);

        let v6 = parse_kernel_reply(&reply, "AAAA", 3);
        assert_eq!(v6.answers, vec!["2001:db8::1"]);
    }

    #[test]
    fn kernel_reply_classifies_failures_instead_of_calling_them_empty() {
        let nx = parse_kernel_reply(&serde_json::json!({ "Status": 3 }), "A", 5);
        assert_eq!(nx.state, "nxdomain");
        assert_eq!(nx.rcode, Some(3));
        let servfail = parse_kernel_reply(&serde_json::json!({ "Status": 2 }), "A", 5);
        assert_eq!(servfail.state, "servfail");
        let unknown = parse_kernel_reply(&serde_json::json!({ "Status": 9 }), "A", 5);
        assert_eq!(unknown.state, "error");
        let malformed = parse_kernel_reply(&serde_json::json!({ "Answer": [] }), "A", 5);
        assert_eq!(malformed.state, "error");
        let empty = parse_kernel_reply(&serde_json::json!({ "Status": 0, "Answer": [] }), "A", 4);
        assert_eq!(empty.state, "no_record");
        // 记录类型是 A，提示里的「没有 A 应答」不能写成 AAAA。
        assert!(empty.note.unwrap().contains("A 应答"));
    }

    fn ok_path(state: &'static str) -> DnsPathResult {
        DnsPathResult {
            state,
            ms: 1,
            rcode: None,
            answers: Vec::new(),
            ttl: None,
            note: None,
        }
    }

    #[test]
    fn verdict_separates_who_could_not_resolve() {
        assert_eq!(verdict(&ok_path("ok"), &ok_path("ok")), "both_ok");
        assert_eq!(verdict(&ok_path("error"), &ok_path("ok")), "kernel_only");
        assert_eq!(verdict(&ok_path("ok"), &ok_path("nxdomain")), "system_only");
        assert_eq!(verdict(&ok_path("timeout"), &ok_path("error")), "both_fail");
        assert_eq!(
            verdict(&ok_path("ok"), &ok_path("unavailable")),
            "kernel_unavailable"
        );
        assert_eq!(ok_path("unavailable").state, "unavailable");
    }

    #[test]
    fn hosts_detection_matches_names_not_substrings() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("hosts");
        std::fs::write(
            &file,
            "127.0.0.1 localhost\n# 10.0.0.1 ignored.example\n10.0.0.2 nas nas.lan\n::1 NAS\n",
        )
        .unwrap();
        assert!(hosts_has(&file, "nas"));
        assert!(hosts_has(&file, "nas.lan"));
        assert!(!hosts_has(&file, "nas.la"));
        assert!(!hosts_has(&file, "ignored.example"));
        assert!(!hosts_has(&dir.path().join("missing"), "nas"));
    }

    #[test]
    fn system_probe_reports_loopback_without_a_live_kernel() {
        let result = system_probe("localhost", "A");
        assert_eq!(result.state, "ok");
        assert!(!result.answers.is_empty());
    }
}
