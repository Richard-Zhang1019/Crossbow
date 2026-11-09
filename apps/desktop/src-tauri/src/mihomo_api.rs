//! mihomo External Controller 客户端（127.0.0.1 TCP + Bearer secret）。
//!
//! REST：模式切换、代理组快照、节点选择、组/单节点延迟测速。
//! WS：traffic/logs/connections 在 ws_bridge（Rust 侧节流聚合）。

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Serialize;

pub const MODES: [&str; 3] = ["direct", "rule", "global"];
/// 测速目标 URL（204 无正文，机场/内核通用基准）。
const BENCHMARK_URL: &str = "http://www.gstatic.com/generate_204";

#[derive(Debug, Clone, Serialize)]
pub struct NodeView {
    pub name: String,
    pub kind: String,
    pub alive: bool,
    /// 最近一次测速延迟（ms）；未测/失败为 None。
    pub delay_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GroupView {
    pub name: String,
    pub kind: String,
    /// 当前选中节点（Selector/URLTest 等）。
    pub now: Option<String>,
    /// URLTest 手动固定节点；空值表示由内核自动选择。
    pub fixed: Option<String>,
    pub nodes: Vec<NodeView>,
}

pub struct Controller {
    pub port: u16,
    pub secret: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutboundInfo {
    pub mode: String,
    /// 最终出口节点名（组会解析到具体节点；DIRECT/REJECT 原样）。
    pub node: String,
    /// 出口节点最近一次测速延迟。
    pub delay_ms: Option<u64>,
    /// 决策链（规则目标组 → 出口）。
    pub chain: Vec<String>,
}

impl Controller {
    fn agent() -> ureq::Agent {
        ureq::AgentBuilder::new().try_proxy_from_env(false)
            .timeout(Duration::from_secs(5))
            .build()
    }

    /// 测速用的长超时 agent（组测速可能到 5s+）。
    fn slow_agent() -> ureq::Agent {
        ureq::AgentBuilder::new().try_proxy_from_env(false)
            .timeout(Duration::from_secs(30))
            .build()
    }

    fn req_on(
        agent: &ureq::Agent,
        port: u16,
        secret: &str,
        method: &str,
        path: &str,
    ) -> ureq::Request {
        agent
            .request(method, &format!("http://127.0.0.1:{port}{path}"))
            .set("Authorization", &format!("Bearer {secret}"))
    }

    fn req(&self, method: &str, path: &str) -> ureq::Request {
        Self::req_on(&Self::agent(), self.port, &self.secret, method, path)
    }

    fn get_json(&self, path: &str) -> Result<serde_json::Value, String> {
        let resp = self
            .req("GET", path)
            .call()
            .map_err(|e| format!("GET {path}: {e}"))?;
        if resp.status() >= 400 {
            return Err(format!("GET {path}: HTTP {}", resp.status()));
        }
        let text = resp
            .into_string()
            .map_err(|e| format!("read {path}: {e}"))?;
        serde_json::from_str(&text).map_err(|e| format!("decode {path}: {e}"))
    }

    #[cfg(any(target_os = "macos", target_os = "windows", test))]
    pub fn tun_enabled(&self) -> Result<Option<bool>, String> {
        Ok(self.get_json("/configs")?["tun"]["enable"].as_bool())
    }

    /// The live device may differ from the configured auto-assigned name.
    /// Never infer ownership from the saved TUN switch or a shared IP range.
    #[cfg(any(target_os = "macos", target_os = "windows", test))]
    pub fn active_tun_device(&self) -> Result<Option<String>, String> {
        let config = self.get_json("/configs")?;
        let tun = &config["tun"];
        if tun["enable"].as_bool() != Some(true) {
            return Ok(None);
        }
        Ok(tun["device"]
            .as_str()
            .filter(|name| {
                if cfg!(target_os = "windows") {
                    *name == "Crossbow"
                } else {
                    name.strip_prefix("utun").is_some_and(|suffix| {
                        !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit())
                    })
                }
            })
            .map(str::to_owned))
    }

    /// 切换运行模式（direct / rule / global）。运行时生效，不落盘。
    /// 热切换 TUN（PATCH /configs）：内核不重启，mihomo 运行时重建
    /// tun 栈。关 TUN 由此绕开「杀 root 内核 → 端口交接 → 重启探测」
    /// 整条链路（实测该链路 30s+ 且 root 内核杀不死时 bind 必失败）。
    /// 仅 mihomo 支持 tun 热更新；sing-box 的 clash_api 不支持。
#[cfg(target_os = "macos")]
    pub fn patch_tun_enable(&self, enable: bool) -> Result<(), String> {
        let body = serde_json::to_vec(&serde_json::json!({
            "tun": { "enable": enable }
        }))
        .map_err(|e| e.to_string())?;
        let resp = self
            .req("PATCH", "/configs")
            .send_bytes(&body)
            .map_err(|e| format!("patch tun: {e}"))?;
        if resp.status() < 300 {
            Ok(())
        } else {
            Err(format!("patch tun: HTTP {}", resp.status()))
        }
    }

    /// 重载已经写好的配置文件，不杀进程。
    ///
    /// mihomo：`PUT /configs?force=true`。路径必须是本次 `-f` 用的 config.yaml，
    /// 它在工作目录内，不需要额外的 SAFE_PATHS。sing-box 的 clash_api 不走这里。
    pub fn reload_config(&self, path: &std::path::Path) -> Result<(), String> {
        let body = serde_json::to_vec(&serde_json::json!({
            "path": path.to_string_lossy(),
            "payload": ""
        }))
        .map_err(|e| e.to_string())?;
        let resp = Self::req_on(
            &Self::reload_agent(),
            self.port,
            &self.secret,
            "PUT",
            "/configs?force=true",
        )
        .send_bytes(&body)
        .map_err(|e| format!("reload config: {e}"))?;
        if resp.status() < 300 {
            Ok(())
        } else {
            Err(format!("reload config: HTTP {}", resp.status()))
        }
    }

    fn reload_agent() -> ureq::Agent {
        ureq::AgentBuilder::new()
            .try_proxy_from_env(false)
            .timeout(Duration::from_secs(60))
            .build()
    }

    pub fn patch_mode(&self, mode: &str) -> Result<(), String> {
        if !MODES.contains(&mode) {
            return Err(format!("unknown mode: {mode}"));
        }
        let body =
            serde_json::to_vec(&serde_json::json!({ "mode": mode })).map_err(|e| e.to_string())?;
        let resp = self
            .req("PATCH", "/configs")
            .send_bytes(&body)
            .map_err(|e| format!("patch mode: {e}"))?;
        if resp.status() < 300 {
            Ok(())
        } else {
            Err(format!("patch mode: HTTP {}", resp.status()))
        }
    }

    /// 代理组快照：过滤出含节点的组（Selector/URLTest/Fallback/LoadBalance），
    /// 组内成员映射为 NodeView（延迟取 history 最新一条）。
    pub fn groups(&self) -> Result<Vec<GroupView>, String> {
        let v = self.get_json("/proxies")?;
        let all = v
            .get("proxies")
            .and_then(|p| p.as_object())
            .ok_or("malformed /proxies")?;
        let mut groups = Vec::new();
        for (name, node) in all {
            let Some(members) = node.get("all").and_then(|a| a.as_array()) else {
                continue; // 普通节点
            };
            let kind = node
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            if matches!(kind.as_str(), "Compatible" | "Direct" | "Reject" | "Pass") {
                continue;
            }
            let now = node.get("now").and_then(|n| n.as_str()).map(String::from);
            let fixed = node
                .get("fixed")
                .and_then(|n| n.as_str())
                .filter(|n| !n.is_empty())
                .map(String::from);
            let nodes = members
                .iter()
                .filter_map(|m| m.as_str())
                .map(|m| {
                    let child = all.get(m);
                    let delay = child
                        .and_then(|c| c.get("history"))
                        .and_then(|h| h.as_array())
                        .and_then(|a| a.last())
                        .and_then(|e| e.get("delay"))
                        .and_then(|d| d.as_u64())
                        .filter(|delay| *delay > 0);
                    // 组嵌组时延迟取组内当前节点不可得，显示为未测即可
                    NodeView {
                        name: m.to_string(),
                        kind: child
                            .and_then(|c| c.get("type"))
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string(),
                        alive: child
                            .and_then(|c| c.get("alive"))
                            .and_then(|a| a.as_bool())
                            .unwrap_or(true),
                        delay_ms: delay,
                    }
                })
                .collect();
            groups.push(GroupView {
                name: name.clone(),
                kind,
                now,
                fixed,
                nodes,
            });
        }
        // 稳定排序：GLOBAL 放最后，其余按名称
        groups.sort_by(|a, b| {
            let ga = (a.name == "GLOBAL") as u8;
            let gb = (b.name == "GLOBAL") as u8;
            ga.cmp(&gb).then(a.name.cmp(&b.name))
        });
        Ok(groups)
    }

    /// 所有页面/托盘选择共用此入口。切回 URLTest 子组时先解除旧固定，
    /// 在 URLTest 本身选择成员则保留内核的手动固定语义。
    pub fn select_proxy(&self, group: &str, name: &str) -> Result<(), String> {
        let source = self.get_json(&build_path(&["/proxies", group]))?;
        if !source["all"]
            .as_array()
            .is_some_and(|members| members.iter().any(|n| n.as_str() == Some(name)))
        {
            return Err("node is not a member of this group".into());
        }
        if source["type"].as_str() == Some("Selector") {
            let target = self.get_json(&build_path(&["/proxies", name]))?;
            if target["type"].as_str() == Some("URLTest")
                && target["fixed"].as_str().is_some_and(|n| !n.is_empty())
            {
                self.reset_proxy_selection(name)?;
            }
        }
        let path = build_path(&["/proxies", group]);
        let body =
            serde_json::to_vec(&serde_json::json!({ "name": name })).map_err(|e| e.to_string())?;
        let resp = self
            .req("PUT", &path)
            .send_bytes(&body)
            .map_err(|e| format!("select {group}: {e}"))?;
        if resp.status() < 300 {
            Ok(())
        } else {
            Err(format!("select {group}: HTTP {}", resp.status()))
        }
    }

    /// mihomo DELETE 清除固定（同时清理内核缓存）；不能用 PUT "" 代替。
    /// 非自动组、未提供 fixed 的兼容内核不发送 DELETE。
    pub fn reset_proxy_selection(&self, group: &str) -> Result<(), String> {
        let path = build_path(&["/proxies", group]);
        let current = self.get_json(&path)?;
        if current["type"].as_str() != Some("URLTest") || !current["fixed"].is_string() {
            return Err("this group does not support restoring automatic selection".into());
        }
        if current["fixed"].as_str() == Some("") {
            return Ok(());
        }
        self.req("DELETE", &path)
            .call()
            .map_err(|e| format!("restore automatic selection: {e}"))?;
        Ok(())
    }

    /// 组测速：mihomo 内部并发探测组内全部节点，返回 {节点: 延迟ms}（失败者缺席）。
    pub fn test_group_delay(&self, group: &str) -> Result<BTreeMap<String, u64>, String> {
        let path = format!(
            "{}?url={}&timeout=5000",
            build_path(&["/group", group, "delay"]),
            BENCHMARK_URL
        );
        let agent = Self::slow_agent();
        let resp = Self::req_on(&agent, self.port, &self.secret, "GET", &path)
            .call()
            .map_err(|e| format!("group delay {group}: {e}"))?;
        if resp.status() >= 400 {
            return Err(format!("group delay {group}: HTTP {}", resp.status()));
        }
        let text = resp.into_string().map_err(|e| e.to_string())?;
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let mut out = BTreeMap::new();
        if let Some(obj) = v.as_object() {
            for (k, d) in obj {
                if let Some(ms) = d.as_u64().filter(|delay| *delay > 0) {
                    out.insert(k.clone(), ms);
                }
            }
        }
        Ok(out)
    }

    /// 控制器版本信息（就绪/诊断用）。
    pub fn version(&self) -> Result<String, String> {
        let v = self.get_json("/version")?;
        Ok(v.get("version")
            .and_then(|x| x.as_str())
            .unwrap_or("unknown")
            .to_string())
    }

    /// 当前出口：规则模式取最后一条 MATCH 规则的目标组并解析其当前节点；
    /// 全局模式取 GLOBAL.now。`final_fallback` 用于 /rules 里没有 MATCH
    /// 规则的内核（sing-box clash api 不合成 MATCH）：读配置的 route.final。
    pub fn current_outbound(&self, final_fallback: Option<&str>) -> Result<OutboundInfo, String> {
        let configs = self.get_json("/configs")?;
        let mode = configs
            .get("mode")
            .and_then(|m| m.as_str())
            .unwrap_or("rule")
            .to_string();
        let proxies_v = self.get_json("/proxies")?;
        let all = proxies_v
            .get("proxies")
            .and_then(|p| p.as_object())
            .ok_or("malformed /proxies")?;

        // 沿 now 链下钻到具体节点，附带其最新延迟。now 为空串（如测速未完成
        // 的 urltest）时停在当前层，不能把链路截断成空名。
        let resolve = |start: &str| -> (String, Option<u64>) {
            let mut cur = start.to_string();
            for _ in 0..6 {
                let Some(node) = all.get(&cur) else { break };
                match node.get("now").and_then(|n| n.as_str()) {
                    Some(next) if !next.is_empty() => cur = next.to_string(),
                    Some(_) => break,
                    None => {
                        let delay = node
                            .get("history")
                            .and_then(|h| h.as_array())
                            .and_then(|a| a.last())
                            .and_then(|e| e.get("delay"))
                            .and_then(|d| d.as_u64())
                            .filter(|delay| *delay > 0);
                        return (cur, delay);
                    }
                }
            }
            (cur, None)
        };

        let (node, delay_ms, chain) = if mode == "global" {
            let (n, d) = resolve("GLOBAL");
            (n, d, vec!["GLOBAL".to_string()])
        } else {
            let rules = self.get_json("/rules")?;
            let match_target = rules
                .get("rules")
                .and_then(|r| r.as_array())
                .and_then(|arr| {
                    arr.iter().rev().find(|r| {
                        r.get("type")
                            .and_then(|t| t.as_str())
                            .map(|t| t.eq_ignore_ascii_case("Match"))
                            .unwrap_or(false)
                    })
                })
                .and_then(|r| r.get("proxy"))
                .and_then(|p| p.as_str())
                .map(String::from);
            let target = match_target
                .or_else(|| final_fallback.map(String::from))
                .unwrap_or_else(|| "DIRECT".to_string());
            if target == "DIRECT" || target == "REJECT" {
                (target.clone(), None, vec![target])
            } else {
                let (n, d) = resolve(&target);
                (n, d, vec![target])
            }
        };
        Ok(OutboundInfo {
            mode,
            node,
            delay_ms,
            chain,
        })
    }

    /// 断开单条连接。
    pub fn close_connection(&self, id: &str) -> Result<(), String> {
        let resp = self
            .req("DELETE", &format!("/connections/{id}"))
            .call()
            .map_err(|e| format!("close connection: {e}"))?;
        if resp.status() < 300 {
            Ok(())
        } else {
            Err(format!("close connection: HTTP {}", resp.status()))
        }
    }

    /// 单节点测速。
    pub fn test_node_delay(&self, node: &str) -> Result<u64, String> {
        let path = format!(
            "{}?url={}&timeout=5000",
            build_path(&["/proxies", node, "delay"]),
            BENCHMARK_URL
        );
        let agent = Self::slow_agent();
        let resp = Self::req_on(&agent, self.port, &self.secret, "GET", &path)
            .call()
            .map_err(|e| format!("node delay {node}: {e}"))?;
        if resp.status() >= 400 {
            return Err(format!("node delay {node}: HTTP {}", resp.status()));
        }
        let text = resp.into_string().map_err(|e| e.to_string())?;
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        v.get("delay")
            .and_then(|d| d.as_u64())
            .filter(|delay| *delay > 0)
            .ok_or_else(|| format!("node delay {node}: no delay field"))
    }
}

/// 拼接并编码路径段（组名常含中文/空格/emoji）。
fn build_path(segments: &[&str]) -> String {
    let mut out = String::new();
    for seg in segments {
        out.push('/');
        for b in seg.trim_start_matches('/').bytes() {
            let c = b as char;
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
                out.push(c);
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A scripted controller checks the actual HTTP contract and request order.
    fn scripted_controller(
        steps: Vec<(&str, String, u16, String)>,
    ) -> (Controller, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let steps: Vec<_> = steps
            .into_iter()
            .map(|(m, p, s, b)| (m.to_owned(), p, s, b))
            .collect();
        let server = std::thread::spawn(move || {
            for (method, path, status, body) in steps {
                let deadline = std::time::Instant::now() + Duration::from_secs(8);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e)
                            if e.kind() == std::io::ErrorKind::WouldBlock
                                && std::time::Instant::now() < deadline =>
                        {
                            std::thread::sleep(Duration::from_millis(5))
                        }
                        Err(e) => panic!("missing {method} {path}: {e}"),
                    }
                };
                // macOS/BSD 的 accept 会让连接继承 listener 的 O_NONBLOCK，
                // 不清掉的话数据未到达时 read 立即返回 WouldBlock（Linux 会清）。
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buf = [0; 1024];
                loop {
                    let n = stream.read(&mut buf).unwrap();
                    assert!(n > 0);
                    request.extend_from_slice(&buf[..n]);
                    if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                        let length = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .and_then(|s| s.parse::<usize>().ok())
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let request = String::from_utf8(request).unwrap();
                assert!(
                    request.starts_with(&format!("{method} {path} HTTP/1.1")),
                    "{request}"
                );
                assert!(request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer test-secret"));
                if method == "PUT" {
                    assert!(serde_json::from_str::<serde_json::Value>(
                        request.split("\r\n\r\n").nth(1).unwrap()
                    )
                    .unwrap()["name"]
                        .is_string());
                }
                write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        (
            Controller {
                port,
                secret: "test-secret".into(),
            },
            server,
        )
    }

    #[test]
    fn selecting_automatic_child_unpins_before_selecting_parent() {
        let parent = build_path(&["/proxies", "节点选择"]);
        let child = build_path(&["/proxies", "♻️ 自动选择"]);
        let source = r#"{"type":"Selector","all":["♻️ 自动选择"]}"#.to_string();
        let target = r#"{"type":"URLTest","fixed":"JP"}"#.to_string();
        let (c, server) = scripted_controller(vec![
            ("GET", parent.clone(), 200, source),
            ("GET", child.clone(), 200, target.clone()),
            ("GET", child.clone(), 200, target),
            ("DELETE", child, 204, "".into()),
            ("PUT", parent, 204, "".into()),
        ]);
        c.select_proxy("节点选择", "♻️ 自动选择").unwrap();
        server.join().unwrap();
    }

    #[test]
    fn selecting_automatic_child_propagates_reset_failure() {
        let (c, server) = scripted_controller(vec![
            (
                "GET",
                "/proxies/main".into(),
                200,
                r#"{"type":"Selector","all":["auto"]}"#.into(),
            ),
            (
                "GET",
                "/proxies/auto".into(),
                200,
                r#"{"type":"URLTest","fixed":"JP"}"#.into(),
            ),
            (
                "GET",
                "/proxies/auto".into(),
                200,
                r#"{"type":"URLTest","fixed":"JP"}"#.into(),
            ),
            ("DELETE", "/proxies/auto".into(), 500, "{}".into()),
        ]);
        assert!(c
            .select_proxy("main", "auto")
            .unwrap_err()
            .contains("restore automatic selection"));
        server.join().unwrap();
    }

    #[test]
    fn automatic_child_without_fixed_support_is_not_deleted() {
        // sing-box Clash API does not expose mihomo's fixed contract.
        let (c, server) = scripted_controller(vec![
            (
                "GET",
                "/proxies/main".into(),
                200,
                r#"{"type":"Selector","all":["auto"]}"#.into(),
            ),
            (
                "GET",
                "/proxies/auto".into(),
                200,
                r#"{"type":"URLTest"}"#.into(),
            ),
            ("PUT", "/proxies/main".into(), 204, "".into()),
        ]);
        c.select_proxy("main", "auto").unwrap();
        server.join().unwrap();
    }

    #[test]
    fn pinning_inside_urltest_does_not_restore_automatic() {
        let (c, server) = scripted_controller(vec![
            (
                "GET",
                "/proxies/auto".into(),
                200,
                r#"{"type":"URLTest","all":["JP"],"fixed":""}"#.into(),
            ),
            ("PUT", "/proxies/auto".into(), 204, "".into()),
        ]);
        c.select_proxy("auto", "JP").unwrap();
        server.join().unwrap();
    }

    #[test]
    fn reset_is_idempotent_and_rejects_unsupported_groups() {
        for (raw, ok) in [
            (r#"{"type":"URLTest","fixed":""}"#, true),
            (r#"{"type":"URLTest"}"#, false),
            (r#"{"type":"Selector","fixed":"JP"}"#, false),
        ] {
            let (c, server) =
                scripted_controller(vec![("GET", "/proxies/auto".into(), 200, raw.into())]);
            assert_eq!(c.reset_proxy_selection("auto").is_ok(), ok);
            server.join().unwrap();
        }
    }

    #[test]
    fn snapshot_preserves_fixed_separately_from_current_outbound() {
        let (c, server) = scripted_controller(vec![(
            "GET",
            "/proxies".into(),
            200,
            r#"{"proxies":{
            "auto":{"type":"URLTest","all":["JP","SG"],"now":"SG","fixed":"JP"},
            "free":{"type":"URLTest","all":["JP"],"now":"JP","fixed":""},
            "JP":{"type":"Direct"},"SG":{"type":"Direct"}}}"#
                .into(),
        )]);
        let groups = c.groups().unwrap();
        let auto = groups.iter().find(|g| g.name == "auto").unwrap();
        assert_eq!(auto.now.as_deref(), Some("SG"));
        assert_eq!(auto.fixed.as_deref(), Some("JP"));
        assert!(groups
            .iter()
            .find(|g| g.name == "free")
            .unwrap()
            .fixed
            .is_none());
        server.join().unwrap();
    }

    #[test]
    fn real_core_urltest_pin_and_restore() {
        let _serial = crate::core_manager::tests::REAL_CORE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Ok(bin) = std::env::var("CROSSBOW_CORE_BIN") else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let mgr = crate::core_manager::CoreManager::new(bin.into(), dir.path().to_path_buf());
        mgr.set_options(crate::core_manager::CoreOptions {
            ensure_geo_files: false,
            ..Default::default()
        });
        let config = "proxies:\n  - {name: OUT-DIRECT, type: direct}\nproxy-groups:\n  - {name: main, type: select, proxies: [auto, OUT-DIRECT]}\n  - {name: auto, type: url-test, proxies: [OUT-DIRECT], url: 'http://127.0.0.1:9', interval: 0}\nrules:\n  - MATCH,DIRECT\n";
        mgr.start(
            config,
            &crossbow_core::RuntimeConfig {
                mixed_port: crate::core_manager::tests::tests_free_port(),
                ..Default::default()
            },
        )
        .unwrap();
        for _ in 0..100 {
            if mgr.status() == crate::core_manager::CoreStatus::Running {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let (port, secret) = mgr.controller().unwrap();
        let c = Controller { port, secret };
        c.select_proxy("auto", "OUT-DIRECT").unwrap();
        assert_eq!(
            c.groups()
                .unwrap()
                .iter()
                .find(|g| g.name == "auto")
                .unwrap()
                .fixed
                .as_deref(),
            Some("OUT-DIRECT")
        );
        c.select_proxy("main", "auto").unwrap();
        assert!(c
            .groups()
            .unwrap()
            .iter()
            .find(|g| g.name == "auto")
            .unwrap()
            .fixed
            .is_none());
        c.select_proxy("auto", "OUT-DIRECT").unwrap();
        c.reset_proxy_selection("auto").unwrap();
        c.reset_proxy_selection("auto").unwrap();
        assert!(c
            .groups()
            .unwrap()
            .iter()
            .find(|g| g.name == "auto")
            .unwrap()
            .fixed
            .is_none());
        mgr.stop();
        // DELETE must clear persistent selection too; a restart must not pin it again.
        mgr.start(
            config,
            &crossbow_core::RuntimeConfig {
                mixed_port: crate::core_manager::tests::tests_free_port(),
                ..Default::default()
            },
        )
        .unwrap();
        for _ in 0..100 {
            if mgr.status() == crate::core_manager::CoreStatus::Running {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let (port, secret) = mgr.controller().unwrap();
        let restarted = Controller { port, secret };
        assert!(restarted
            .groups()
            .unwrap()
            .iter()
            .find(|g| g.name == "auto")
            .unwrap()
            .fixed
            .is_none());
        mgr.stop();
    }

    #[test]
    fn tun_device_requires_enabled_live_interface() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        for (body, expected, enabled) in [
            (
                r#"{"tun":{"enable":true,"device":"utun4"}}"#,
                (!cfg!(target_os = "windows")).then_some("utun4"),
                Some(true),
            ),
            (
                r#"{"tun":{"enable":false,"device":"utun4"}}"#,
                None,
                Some(false),
            ),
            (
                r#"{"tun":{"enable":true,"device":"utun"}}"#,
                None,
                Some(true),
            ),
            (
                r#"{"tun":{"enable":true,"device":"en0"}}"#,
                None,
                Some(true),
            ),
            (r#"{"tun":{"enable":true,"device":"Crossbow"}}"#,
                cfg!(target_os = "windows").then_some("Crossbow"), Some(true)),
            (r#"{"tun":{"enable":true}}"#, None, Some(true)),
            (r#"{}"#, None, None),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let server = std::thread::spawn(move || {
                for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                // macOS/BSD 的 accept 会让连接继承 listener 的 O_NONBLOCK，
                // 不清掉的话数据未到达时 read 立即返回 WouldBlock（Linux 会清）。
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buf = [0; 512];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = stream.read(&mut buf).unwrap();
                    assert!(n > 0);
                    request.extend_from_slice(&buf[..n]);
                }
                let request = String::from_utf8(request).unwrap();
                assert!(request.starts_with("GET /configs HTTP/1.1"));
                assert!(request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer test-secret"));
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                }
            });
            let client = Controller {
                port,
                secret: "test-secret".into(),
            };
            assert_eq!(client.active_tun_device().unwrap().as_deref(), expected);
            assert_eq!(client.tun_enabled().unwrap(), enabled);
            server.join().unwrap();
        }
    }

    #[test]
    fn path_encoding_handles_cjk_and_spaces() {
        assert_eq!(
            build_path(&["/proxies", "香港 自动"]),
            "/proxies/%E9%A6%99%E6%B8%AF%20%E8%87%AA%E5%8A%A8"
        );
        assert_eq!(build_path(&["/proxies", "HK-01"]), "/proxies/HK-01");
    }

    #[test]
    fn groups_filters_and_maps_delay() {
        // groups() 需要 HTTP；这里只测路径构建与解析辅助——延迟映射逻辑内联验证
        let raw = serde_json::json!({
            "proxies": {
                "节点选择": { "type": "Selector", "now": "HK-01", "all": ["HK-01", "US-01"] },
                "HK-01": { "type": "Shadowsocks", "alive": true,
                           "history": [{"time": "t1", "delay": 128}] },
                "US-01": { "type": "Trojan", "alive": false, "history": [] },
                "DIRECT": { "type": "Direct" }
            }
        });
        let all = raw.get("proxies").unwrap().as_object().unwrap();
        let names: Vec<_> = all.keys().cloned().collect();
        assert!(names.contains(&"节点选择".to_string()));
        let delay = all["HK-01"]["history"].as_array().unwrap().last().unwrap()["delay"].as_u64();
        assert_eq!(delay, Some(128));
    }

    /// 真实内核：组快照 → 选择 → 单节点测速 全链路。
    #[test]
    fn real_core_groups_select_and_delay() {
        let _serial = crate::core_manager::tests::REAL_CORE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Ok(bin) = std::env::var("CROSSBOW_CORE_BIN") else {
            eprintln!("skip: CROSSBOW_CORE_BIN not set");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let mgr = crate::core_manager::CoreManager::new(
            std::path::PathBuf::from(bin),
            dir.path().to_path_buf(),
        );
        mgr.set_options(crate::core_manager::CoreOptions {
            ensure_geo_files: false,
            ..crate::core_manager::CoreOptions::default()
        });
        let config = "proxies:\n  - name: OUT-DIRECT\n    type: direct\nproxy-groups:\n  - name: \u{8282}\u{70b9}\u{9009}\u{62e9}\n    type: select\n    proxies:\n      - OUT-DIRECT\nrules:\n  - MATCH,DIRECT\n";
        mgr.start(
            config,
            &crossbow_core::RuntimeConfig {
                mixed_port: crate::core_manager::tests::tests_free_port(),
                ..crossbow_core::RuntimeConfig::default()
            },
        )
        .unwrap();
        let mut running = false;
        for _ in 0..100 {
            if mgr.status() == crate::core_manager::CoreStatus::Running {
                running = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(running);

        let (port, secret) = mgr.controller().unwrap();
        let c = Controller { port, secret };

        let groups = c.groups().unwrap();
        let sel = groups
            .iter()
            .find(|g| g.name == "\u{8282}\u{70b9}\u{9009}\u{62e9}")
            .expect("selector group present");
        assert_eq!(sel.nodes.len(), 1);
        assert_eq!(sel.nodes[0].name, "OUT-DIRECT");

        c.select_proxy("\u{8282}\u{70b9}\u{9009}\u{62e9}", "OUT-DIRECT")
            .unwrap();

        let delay = c.test_node_delay("OUT-DIRECT").unwrap();
        assert!(delay > 0, "direct adapter should answer the probe");

        mgr.stop();
    }
}
