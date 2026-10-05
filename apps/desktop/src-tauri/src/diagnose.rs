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
    /// 技术细节（直接展示的补充信息）。
    pub detail: String,
    /// 可执行修复动作 id：start_core / restart_core / toggle_sysproxy。
    pub fix: Option<String>,
}

fn ok(id: &str, detail: impl Into<String>) -> CheckResult {
    CheckResult {
        id: id.into(),
        ok: true,
        detail: detail.into(),
        fix: None,
    }
}

fn bad(id: &str, detail: impl Into<String>, fix: Option<&str>) -> CheckResult {
    CheckResult {
        id: id.into(),
        ok: false,
        detail: detail.into(),
        fix: fix.map(String::from),
    }
}

pub fn run(state: &AppState) -> Vec<CheckResult> {
    let mut results = Vec::new();
    let mixed_port = state.store.lock().unwrap().data().engine.mixed_port;

    // 1. 内核状态
    let status = state.core().status();
    match &status {
        crate::core_manager::CoreStatus::Running => {
            results.push(ok("core_running", "内核进程运行中"));
        }
        crate::core_manager::CoreStatus::Starting => {
            results.push(bad("core_running", "内核正在启动", Some("start_core")));
        }
        crate::core_manager::CoreStatus::Crashed(msg) => {
            results.push(bad("core_running", msg.clone(), Some("restart_core")));
        }
        crate::core_manager::CoreStatus::Stopped => {
            results.push(bad("core_running", "内核未运行", Some("start_core")));
        }
    }

    // 2. 控制器 API
    if let Some((port, secret)) = state.core().controller() {
        let c = crate::mihomo_api::Controller { port, secret };
        match c.version() {
            Ok(v) => results.push(ok("controller", v)),
            Err(e) => results.push(bad("controller", e, Some("restart_core"))),
        }
    } else if matches!(status, crate::core_manager::CoreStatus::Running) {
        results.push(bad(
            "controller",
            "运行中但控制器信息缺失",
            Some("restart_core"),
        ));
    }

    // 3. 配置可渲染
    {
        let store = state.store.lock().unwrap();
        match crossbow_core::render_config(store.data()) {
            Ok(r) => results.push(ok("config_renders", format!("profile {}", r.profile_id))),
            Err(e) => results.push(bad("config_renders", e.to_string(), None)),
        }
    }

    // 4. 混合端口监听
    match std::net::TcpStream::connect_timeout(
        &format!("127.0.0.1:{mixed_port}").parse().unwrap(),
        Duration::from_millis(1200),
    ) {
        Ok(_) => results.push(ok("listener", format!("127.0.0.1:{mixed_port} 可连接"))),
        Err(e) => results.push(bad("listener", format!("{e}"), Some("restart_core"))),
    }

    // 5. 系统代理指向
    let endpoints = state.sysproxy.external_endpoints();
    if endpoints.is_empty() {
        results.push(bad(
            "sysproxy",
            "系统代理未开启（浏览器流量不会经过本应用）",
            Some("toggle_sysproxy"),
        ));
    } else {
        let ours = endpoints
            .iter()
            .any(|(_, ep)| ep.host == "127.0.0.1" && ep.port == mixed_port);
        let others: Vec<String> = endpoints
            .iter()
            .filter(|(_, ep)| !(ep.host == "127.0.0.1" && ep.port == mixed_port))
            .map(|(svc, ep)| format!("{svc}→{}:{}", ep.host, ep.port))
            .collect();
        if ours {
            results.push(ok("sysproxy", format!("系统代理指向本应用 :{mixed_port}")));
        } else {
            results.push(bad(
                "sysproxy",
                format!(
                    "系统代理指向 {}（非本应用，可能存在第三方客户端）",
                    others.join(", ")
                ),
                Some("toggle_sysproxy"),
            ));
        }
    }

    // 6. 代理连通性（经混合端口访问 204 端点）
    match proxied_probe(mixed_port) {
        Ok(()) => results.push(ok("proxied_traffic", "经内核访问外网正常（HTTP 204）")),
        Err(e) => results.push(bad("proxied_traffic", e, None)),
    }

    // 7. 直连 DNS
    match ("www.gstatic.com", 80u16).to_socket_addrs() {
        Ok(mut addrs) => {
            let first = addrs.next().map(|a| a.ip().to_string()).unwrap_or_default();
            results.push(ok("dns_direct", format!("解析正常（{first}）")));
        }
        Err(e) => results.push(bad("dns_direct", format!("系统 DNS 解析失败：{e}"), None)),
    }

    results
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
