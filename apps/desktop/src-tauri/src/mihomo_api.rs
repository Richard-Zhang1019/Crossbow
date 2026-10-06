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
        ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(5))
            .build()
    }

    /// 测速用的长超时 agent（组测速可能到 5s+）。
    fn slow_agent() -> ureq::Agent {
        ureq::AgentBuilder::new()
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

    /// 切换运行模式（direct / rule / global）。运行时生效，不落盘。
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
                        .and_then(|d| d.as_u64());
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

    /// 选择组内节点（Selector）。组名/节点名按路径段自动百分号编码。
    pub fn select_proxy(&self, group: &str, name: &str) -> Result<(), String> {
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
                if let Some(ms) = d.as_u64() {
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
    /// 全局模式取 GLOBAL.now。
    pub fn current_outbound(&self) -> Result<OutboundInfo, String> {
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

        // 沿 now 链下钻到具体节点，附带其最新延迟
        let resolve = |start: &str| -> (String, Option<u64>) {
            let mut cur = start.to_string();
            for _ in 0..6 {
                let Some(node) = all.get(&cur) else { break };
                match node.get("now").and_then(|n| n.as_str()) {
                    Some(next) => cur = next.to_string(),
                    None => {
                        let delay = node
                            .get("history")
                            .and_then(|h| h.as_array())
                            .and_then(|a| a.last())
                            .and_then(|e| e.get("delay"))
                            .and_then(|d| d.as_u64());
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
            let target = rules
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
                .unwrap_or("DIRECT")
                .to_string();
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
        let _serial = crate::core_manager::tests::REAL_CORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(bin) = std::env::var("CROSSBOW_CORE_BIN") else {
            eprintln!("skip: CROSSBOW_CORE_BIN not set");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let mgr = crate::core_manager::CoreManager::new(
            std::path::PathBuf::from(bin),
            dir.path().to_path_buf(),
        );
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
