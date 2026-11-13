//! 网络诊断向导（M1.2）：按依赖顺序自检，每项给出结论与可执行的修复动作。
//!
//! 检查项：内核状态 → 控制器可达 → 配置可渲染 → 混合端口监听 →
//! 系统代理指向 → 代理连通性（经内核访问 204 端点）→ 直连 DNS。

use std::net::ToSocketAddrs;
use std::time::Duration;

use serde::Serialize;

use crate::AppState;

#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    /// 稳定 id，前端据此取本地化标题。
    pub id: String,
    pub ok: bool,
    /// Unsupported checks are excluded, never counted as passed.
    pub skipped: bool,
    /// 技术细节（直接展示的补充信息）。
    pub detail: String,
    /// 可执行修复动作 id：start_core / restart_core / enable_sysproxy。
    pub fix: Option<String>,
}

fn ok(id: &str, detail: impl Into<String>) -> CheckResult {
    CheckResult {
        id: id.into(),
        ok: true,
        skipped: false,
        detail: detail.into(),
        fix: None,
    }
}

fn bad(id: &str, detail: impl Into<String>, fix: Option<&str>) -> CheckResult {
    CheckResult {
        id: id.into(),
        ok: false,
        skipped: false,
        detail: detail.into(),
        fix: fix.map(String::from),
    }
}

#[derive(Clone, Serialize)]
pub struct Progress {
    pub run_id: String,
    pub id: String,
    pub result: Option<CheckResult>,
}
struct Report<'a> {
    rows: Vec<CheckResult>,
    run_id: &'a str,
    send: &'a dyn Fn(Progress),
}
impl Report<'_> {
    fn start(&self, id: &str) {
        (self.send)(Progress {
            run_id: self.run_id.into(),
            id: id.into(),
            result: None,
        });
    }
    fn push(&mut self, mut row: CheckResult) {
        row.detail = crate::notifications::redact(&row.detail);
        (self.send)(Progress {
            run_id: self.run_id.into(),
            id: row.id.clone(),
            result: Some(row.clone()),
        });
        self.rows.push(row);
    }
    fn skip(&mut self, id: &str) {
        self.push(CheckResult {
            id: id.into(),
            ok: false,
            skipped: true,
            detail: "Dependency is not ready".into(),
            fix: None,
        });
    }
}
pub fn run(state: &AppState, run_id: &str, send: impl Fn(Progress)) -> DiagnosticReport {
    let mut results = Report {
        rows: Vec::new(),
        run_id,
        send: &send,
    };
    let (mixed_port, tun_enabled, engine) = {
        let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        let settings = &store.data().engine;
        let engine = match settings.engine {
            crossbow_core::Engine::Mihomo => "mihomo",
            crossbow_core::Engine::SingBox => "sing-box",
        };
        (settings.mixed_port, settings.tun_enable, engine)
    };

    // 1. 内核状态
    results.start("core_running");
    let status = state.core().status();
    match &status {
        crate::engine::manager::CoreStatus::Running => {
            results.push(ok("core_running", "内核进程运行中"));
        }
        crate::engine::manager::CoreStatus::Starting => {
            results.push(bad("core_running", "内核正在启动", Some("start_core")));
        }
        crate::engine::manager::CoreStatus::Crashed(msg) => {
            results.push(bad("core_running", msg.clone(), Some("restart_core")));
        }
        crate::engine::manager::CoreStatus::Stopped => {
            results.push(bad("core_running", "内核未运行", Some("start_core")));
        }
    }

    // 2. 控制器 API
    results.start("controller");
    if let Some((port, secret)) = state.core().controller() {
        let c = crate::engine::api::Controller { port, secret };
        match c.version() {
            Ok(v) => results.push(ok("controller", v)),
            Err(e) => results.push(bad("controller", e, Some("restart_core"))),
        }
    } else if matches!(status, crate::engine::manager::CoreStatus::Running) {
        results.push(bad(
            "controller",
            "运行中但控制器信息缺失",
            Some("restart_core"),
        ));
    }

    if !results.rows.iter().any(|r| r.id == "controller") {
        results.skip("controller");
    }

    // 3. 配置可渲染
    results.start("config_renders");
    {
        let store = state.store.lock().unwrap();
        match crossbow_core::render_config(store.data()) {
            Ok(r) => results.push(ok("config_renders", format!("profile {}", r.profile_id))),
            Err(e) => results.push(bad("config_renders", e.to_string(), None)),
        }
    }

    // 4. 混合端口监听
    results.start("listener");
    if !matches!(status, crate::engine::manager::CoreStatus::Running) {
        results.skip("listener");
    } else {
        match std::net::TcpStream::connect_timeout(
            &format!("127.0.0.1:{mixed_port}").parse().unwrap(),
            Duration::from_millis(1200),
        ) {
            Ok(_) => results.push(ok("listener", format!("127.0.0.1:{mixed_port} 可连接"))),
            Err(e) => results.push(bad("listener", format!("{e}"), Some("restart_core"))),
        }
    }
    // System proxy settings and TUN capture are independent of the explicit HTTP probe.
    results.start("sysproxy");
    let (proxy, mismatch) = if crate::platform::capabilities().system_proxy {
        use crate::network::sysproxy::ProxyInspection;
        match state.sysproxy.inspect(mixed_port) {
            Ok(ProxyInspection::Configured) => ("configured", None),
            Ok(ProxyInspection::Off) => ("off", None),
            Ok(ProxyInspection::Mismatch { http, https, socks }) => {
                let protocols = [("HTTP", http), ("HTTPS", https), ("SOCKS", socks)]
                    .into_iter()
                    .filter_map(|(protocol, matches)| (!matches).then_some(protocol))
                    .collect::<Vec<_>>();
                let detail = if protocols.is_empty() {
                    "检测到自动代理设置，无法确认流量经过本应用".into()
                } else {
                    format!("{} 未全部指向本应用 :{mixed_port}", protocols.join(" / "))
                };
                ("mismatch", Some(detail))
            }
            Err(_) => ("unknown", None),
        }
    } else {
        ("unsupported", None)
    };
    let tun = inspect_tun(state, tun_enabled);
    results.push(takeover_result(proxy, tun, &status, mixed_port, mismatch));

    // 6. 代理连通性（经混合端口访问 204 端点）
    results.start("proxied_traffic");
    if !matches!(status, crate::engine::manager::CoreStatus::Running) {
        results.skip("proxied_traffic");
    } else {
        match proxied_probe(mixed_port) {
            Ok(()) => results.push(ok(
                "proxied_traffic",
                "经混合端口显式 HTTP 代理访问测试站点成功（HTTP 204）",
            )),
            Err(e) => results.push(bad("proxied_traffic", e, None)),
        }
    }
    // 7. 直连 DNS
    results.start("dns_direct");
    match ("www.gstatic.com", 80u16).to_socket_addrs() {
        Ok(mut addrs) => {
            let first = addrs.next().map(|a| a.ip().to_string()).unwrap_or_default();
            results.push(ok("dns_direct", format!("解析正常（{first}）")));
        }
        Err(e) => results.push(bad("dns_direct", format!("系统 DNS 解析失败：{e}"), None)),
    }

    let core_version = results
        .rows
        .iter()
        .find(|row| row.id == "controller" && row.ok)
        .and_then(|row| {
            let version = row.detail.trim().trim_start_matches('v');
            (version.len() <= 40
                && !version.is_empty()
                && version
                    .bytes()
                    .all(|c| c.is_ascii_digit() || matches!(c, b'.' | b'-' | b'+')))
            .then(|| version.to_owned())
        });
    let os_version = os_info::get().version().to_string();
    let os_version = os_version
        .bytes()
        .all(|c| c.is_ascii_digit() || c == b'.')
        .then_some(os_version);
    DiagnosticReport {
        rows: results.rows,
        context: DiagnosticContext {
            os: std::env::consts::OS,
            os_version,
            engine,
            core_state: match status {
                crate::CoreStatus::Running => "running",
                crate::CoreStatus::Starting => "starting",
                crate::CoreStatus::Stopped => "stopped",
                crate::CoreStatus::Crashed(_) => "crashed",
            },
            core_version,
            proxy,
            tun,
            mixed_port,
        },
    }
}

#[derive(Serialize)]
pub struct DiagnosticReport {
    rows: Vec<CheckResult>,
    context: DiagnosticContext,
}

#[derive(Serialize)]
struct DiagnosticContext {
    os: &'static str,
    os_version: Option<String>,
    engine: &'static str,
    core_state: &'static str,
    core_version: Option<String>,
    proxy: &'static str,
    tun: &'static str,
    mixed_port: u16,
}

fn takeover_result(
    proxy: &str,
    tun: &str,
    status: &crate::CoreStatus,
    port: u16,
    mismatch: Option<String>,
) -> CheckResult {
    let result = match proxy {
        "configured" => ok(
            "sysproxy",
            format!("系统代理端点指向本应用 :{port}；未验证浏览器是否使用该设置"),
        ),
        "off" if tun == "ready" => ok("sysproxy", "系统代理已关闭，Crossbow 的 TUN 接管已就绪"),
        "off" if tun == "off" => bad(
            "sysproxy",
            "系统代理未开启；仍可通过混合端口使用显式代理",
            (*status == crate::CoreStatus::Running).then_some("enable_sysproxy"),
        ),
        "off" | "unsupported" => CheckResult {
            id: "sysproxy".into(),
            ok: false,
            skipped: true,
            detail: "系统代理未使用".into(),
            fix: None,
        },
        "mismatch" => bad(
            "sysproxy",
            mismatch.unwrap_or_else(|| "系统代理端点不匹配".into()),
            None,
        ),
        _ => bad("sysproxy", "系统代理读取失败，无法验证端点", None),
    };
    let tun_detail = match tun {
        "ready" if proxy != "off" => Some(("Crossbow 的 TUN 接管已就绪", false)),
        "inactive" => Some(("TUN 已开启，但 Crossbow 的捕获路由或适配器未就绪", true)),
        "unknown" => Some(("无法验证 Crossbow 的 TUN 接管状态", true)),
        _ => None,
    };
    match tun_detail {
        Some((detail, failed)) if failed => {
            bad("sysproxy", format!("{}；{detail}", result.detail), None)
        }
        Some((detail, _)) => CheckResult {
            ok: result.ok || result.skipped,
            skipped: false,
            detail: format!("{}；{detail}", result.detail),
            ..result
        },
        None => result,
    }
}

fn inspect_tun(state: &AppState, enabled: bool) -> &'static str {
    if !enabled {
        return "off";
    }
    if state.core().status() != crate::CoreStatus::Running {
        return "inactive";
    }
    #[cfg(any(target_os = "macos", windows))]
    let ready =
        state.core().controller().and_then(
            |(port, secret)| match (crate::engine::api::Controller { port, secret })
                .active_tun_device()
            {
                Ok(Some(device)) => {
                    #[cfg(target_os = "macos")]
                    return crate::network::tun::diagnostic_capture_route(&device);
                    #[cfg(windows)]
                    return crate::network::tun_windows::diagnostic_adapter_ready(&device);
                }
                Ok(None) => Some(false),
                Err(_) => None,
            },
        );
    #[cfg(target_os = "linux")]
    let ready = crate::network::linux_service::status()
        .ok()
        .map(|status| status.is_some_and(|status| status.active));
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    let ready = None;
    match ready {
        Some(true) => "ready",
        Some(false) => "inactive",
        None => "unknown",
    }
}

/// 经混合端口探测 204 端点。
fn proxied_probe(mixed_port: u16) -> Result<(), String> {
    let proxy = ureq::Proxy::new(format!("http://127.0.0.1:{mixed_port}"))
        .map_err(|e| format!("proxy 配置错误：{e}"))?;
    let agent = ureq::AgentBuilder::new()
        .proxy(proxy)
        .timeout(Duration::from_secs(8))
        .build();
    match agent.get("http://www.gstatic.com/generate_204").call() {
        Ok(resp) if resp.status() == 204 => Ok(()),
        Ok(resp) => Err(format!("探测返回 HTTP {}", resp.status())),
        Err(e) => Err(format!("经内核访问外网失败：{e}")),
    }
}

#[cfg(test)]
mod progress_tests {
    #[test]
    fn a_live_tun_does_not_mask_bad_or_unknown_system_proxy_settings() {
        let running = crate::CoreStatus::Running;
        let tun_only = super::takeover_result("off", "ready", &running, 7898, None);
        assert!(tun_only.ok && tun_only.fix.is_none());
        for proxy in ["mismatch", "unknown"] {
            let row = super::takeover_result(proxy, "ready", &running, 7898, None);
            assert!(!row.ok && !row.skipped && row.fix.is_none());
        }
        let row = super::takeover_result("configured", "inactive", &running, 7898, None);
        assert!(!row.ok && row.fix.is_none());
    }

    #[test]
    fn reports_start_and_skip_with_the_original_run_id() {
        let events = std::cell::RefCell::new(Vec::new());
        let send = |p| events.borrow_mut().push(p);
        let mut report = super::Report {
            rows: Vec::new(),
            run_id: "attempt-1",
            send: &send,
        };
        report.start("controller");
        report.skip("controller");
        let e = events.borrow();
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].run_id, "attempt-1");
        assert!(e[0].result.is_none());
        assert!(e[1].result.as_ref().unwrap().skipped);
    }
}
