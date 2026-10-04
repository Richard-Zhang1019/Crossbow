//! mihomo External Controller 客户端（127.0.0.1 TCP + Bearer secret）。
//!
//! S3 只需要模式切换；traffic/logs/connections 的 WS 订阅在 S4 加入，
//! 届时连接保活与节流聚合都在 Rust 侧做（DESIGN §5.2）。

use std::time::Duration;

pub const MODES: [&str; 3] = ["direct", "rule", "global"];

pub struct Controller {
    pub port: u16,
    pub secret: String,
}

impl Controller {
    fn agent() -> ureq::Agent {
        ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(5))
            .build()
    }

    fn req(&self, method: &str, path: &str) -> ureq::Request {
        Self::agent()
            .request(method, &format!("http://127.0.0.1:{}{path}", self.port))
            .set("Authorization", &format!("Bearer {}", self.secret))
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
}
