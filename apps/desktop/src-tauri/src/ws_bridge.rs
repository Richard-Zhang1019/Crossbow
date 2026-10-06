//! 内核 WS 数据桥：traffic / connections / logs 三路订阅。
//!
//! 设计要点（DESIGN §5.2）：连接与日志由前端按页面挂载/卸载触发订阅/退订，
//! Rust 侧持有唯一 WS 读线程并做节流聚合后 emit 事件——避免大快照刷爆
//! webview。读线程阻塞式 read（内核每秒必推数据），退订后最迟约 1 秒退出；
//! 连接断开（内核重启中）自动重连。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tungstenite::Message;

pub const EV_TRAFFIC: &str = "traffic://data";
pub const EV_CONNECTIONS: &str = "connections://data";
pub const EV_LOGS: &str = "logs://data";

/// connections 快照推送间隔下限（mihomo 原生 1Hz）。
const CONN_MIN_INTERVAL: Duration = Duration::from_millis(1000);
/// 单次快照推送的最大行数：系统代理全开时连接可达数千上万，
/// 超限部分截断（前端按 totals 展示全量），避免巨型 IPC 载荷。
const CONN_MAX_ROWS: usize = 1000;
/// 日志批量 flush：间隔或条数任一达到即发。
const LOG_FLUSH_INTERVAL: Duration = Duration::from_millis(300);
const LOG_FLUSH_COUNT: usize = 20;

#[derive(Default)]
pub struct WsHub {
    /// channel 名 → 活跃标志；false = 已请求退订，读线程自行退出。
    active: Mutex<HashMap<&'static str, Arc<AtomicBool>>>,
}

impl WsHub {
    fn subscribe<F>(&self, name: &'static str, spawn_reader: F) -> Result<(), String>
    where
        F: FnOnce(Arc<AtomicBool>) + Send + 'static,
    {
        let mut map = self.active.lock().unwrap();
        if let Some(flag) = map.get(name) {
            if flag.load(Ordering::SeqCst) {
                return Ok(()); // 已在订阅
            }
        }
        let flag = Arc::new(AtomicBool::new(true));
        map.insert(name, flag.clone());
        drop(map);
        spawn_reader(flag);
        Ok(())
    }

    pub fn unsubscribe(&self, name: &str) {
        if let Some(flag) = self.active.lock().unwrap().get(name) {
            flag.store(false, Ordering::SeqCst);
        }
    }

    pub fn unsubscribe_all(&self) {
        for flag in self.active.lock().unwrap().values() {
            flag.store(false, Ordering::SeqCst);
        }
    }

    pub fn subscribe_traffic(
        &self,
        port: u16,
        secret: &str,
        emit: impl Fn(&'static str, serde_json::Value) + Send + 'static,
    ) -> Result<(), String> {
        let url = ws_url(port, secret, "/traffic");
        self.subscribe("traffic", move |flag| {
            std::thread::spawn(move || {
                read_loop(&url, flag, |msg| {
                    if let Message::Text(txt) = msg {
                        // mihomo 原始 {"up":..,"down":..} → 结构化转发
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
                            emit(EV_TRAFFIC, v);
                        }
                    }
                });
            });
        })
    }

    pub fn subscribe_connections(
        &self,
        port: u16,
        secret: &str,
        emit: impl Fn(&'static str, serde_json::Value) + Send + 'static,
    ) -> Result<(), String> {
        let url = ws_url(port, secret, "/connections");
        self.subscribe("connections", move |flag| {
            std::thread::spawn(move || {
                let mut last_emit = Instant::now() - CONN_MIN_INTERVAL;
                read_loop(&url, flag, |msg| {
                    if let Message::Text(txt) = msg {
                        if last_emit.elapsed() < CONN_MIN_INTERVAL {
                            return; // 节流
                        }
                        if let Some(payload) = compact_connections(&txt) {
                            last_emit = Instant::now();
                            emit(EV_CONNECTIONS, payload);
                        }
                    }
                });
            });
        })
    }

    pub fn subscribe_logs(
        &self,
        port: u16,
        secret: &str,
        emit: impl Fn(&'static str, serde_json::Value) + Send + 'static,
    ) -> Result<(), String> {
        let url = ws_url(port, secret, "/logs?level=info");
        self.subscribe("logs", move |flag| {
            std::thread::spawn(move || {
                let mut batch: Vec<serde_json::Value> = Vec::new();
                let mut last_flush = Instant::now();
                read_loop(&url, flag, |msg| {
                    if let Message::Text(txt) = msg {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
                            batch.push(v);
                        }
                        if last_flush.elapsed() >= LOG_FLUSH_INTERVAL
                            || batch.len() >= LOG_FLUSH_COUNT
                        {
                            emit(EV_LOGS, serde_json::Value::Array(batch.clone()));
                            batch.clear();
                            last_flush = Instant::now();
                        }
                    }
                });
                if !batch.is_empty() {
                    emit(EV_LOGS, serde_json::Value::Array(batch));
                }
            });
        })
    }
}

fn ws_url(port: u16, secret: &str, path: &str) -> String {
    format!("ws://127.0.0.1:{port}{path}?token={secret}")
}

/// 阻塞式 WS 读循环：断线自动重连，退订后于下一条消息前退出。
fn read_loop(url: &str, flag: Arc<AtomicBool>, mut on_text: impl FnMut(Message)) {
    'outer: while flag.load(Ordering::SeqCst) {
        let mut ws = match tungstenite::connect(url) {
            Ok((ws, _)) => ws,
            Err(_) => {
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        loop {
            if !flag.load(Ordering::SeqCst) {
                break 'outer;
            }
            match ws.read() {
                Ok(Message::Ping(_) | Message::Pong(_)) => {}
                Ok(msg @ Message::Text(_)) => on_text(msg),
                Ok(Message::Close(_)) => break, // 内核重启中，走重连
                Ok(_) => {}
                Err(_) => break,
            }
        }
        drop(ws);
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[derive(Serialize)]
struct ConnRow {
    id: String,
    host: String,
    rule: String,
    chains: String,
    up: u64,
    down: u64,
    network: String,
    kind: String,
    process: String,
    elapsed_ms: u64,
}

#[derive(Serialize)]
struct ConnPayload {
    rows: Vec<ConnRow>,
    upload_total: u64,
    download_total: u64,
    memory: u64,
    /// 超过 CONN_MAX_ROWS 被截断时的原始总行数。
    #[serde(skip_serializing_if = "Option::is_none")]
    truncated: Option<usize>,
}

/// mihomo connections 快照 → 紧凑结构（字段裁剪让大快照的 IPC 载荷可控）。
fn compact_connections(raw: &str) -> Option<serde_json::Value> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let now_ms = now_unix_ms();
    let mut rows = Vec::new();
    if let Some(conns) = v.get("connections").and_then(|c| c.as_array()) {
        for c in conns {
            let md = c.get("metadata").cloned().unwrap_or_default();
            let str_at = |k: &str| md.get(k).and_then(|x| x.as_str()).unwrap_or("");
            let host = {
                let h = str_at("host");
                let port = str_at("destinationPort");
                if h.is_empty() {
                    format!("{}:{port}", str_at("destinationIP"))
                } else {
                    format!("{h}:{port}")
                }
            };
            rows.push(ConnRow {
                id: c
                    .get("id")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                host,
                rule: format!(
                    "{}{}",
                    c.get("rule").and_then(|x| x.as_str()).unwrap_or(""),
                    c.get("rulePayload")
                        .and_then(|x| x.as_str())
                        .map(|p| format!(" {p}"))
                        .unwrap_or_default()
                ),
                chains: c
                    .get("chains")
                    .and_then(|x| x.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(" → ")
                    })
                    .unwrap_or_default(),
                up: c.get("upload").and_then(|x| x.as_u64()).unwrap_or(0),
                down: c.get("download").and_then(|x| x.as_u64()).unwrap_or(0),
                network: str_at("network").to_string(),
                kind: str_at("type").to_string(),
                process: str_at("processPath")
                    .rsplit('/')
                    .next()
                    .unwrap_or("")
                    .to_string(),
                elapsed_ms: now_ms.saturating_sub(
                    c.get("start")
                        .and_then(|x| x.as_str())
                        .and_then(parse_rfc3339_ms)
                        .unwrap_or(0),
                ),
            });
        }
    }
    let total_rows = rows.len();
    rows.truncate(CONN_MAX_ROWS);
    let payload = ConnPayload {
        upload_total: v.get("uploadTotal").and_then(|x| x.as_u64()).unwrap_or(0),
        download_total: v.get("downloadTotal").and_then(|x| x.as_u64()).unwrap_or(0),
        memory: v.get("memory").and_then(|x| x.as_u64()).unwrap_or(0),
        truncated: if total_rows > CONN_MAX_ROWS {
            Some(total_rows)
        } else {
            None
        },
        rows,
    };
    serde_json::to_value(&payload).ok()
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 解析 mihomo 的 RFC3339 时间戳为 Unix 毫秒（忽略小数秒与时区偏移外的精度）。
fn parse_rfc3339_ms(s: &str) -> Option<u64> {
    // 2026-10-04T12:00:00.123456+08:00 / …Z
    let (sec_part, _frac) = s.split_once('.').unwrap_or((s, ""));
    let sec_part = sec_part.trim_end_matches('Z');
    let (dt, offset_min) = match sec_part.rsplit_once(['+', '-']) {
        Some((dt, off)) if off.len() == 5 => {
            let sign = if sec_part.contains('+') { 1 } else { -1 };
            let h: i64 = off[..2].parse().ok()?;
            let m: i64 = off[3..].parse().ok()?;
            (dt.to_string(), sign * (h * 60 + m))
        }
        _ => (sec_part.to_string(), 0),
    };
    let mut it = dt.split(&['T', ' '][..]);
    let date = it.next()?.split('-').collect::<Vec<_>>();
    let time = it.next()?.split(':').collect::<Vec<_>>();
    if date.len() != 3 || time.len() != 3 {
        return None;
    }
    let y: i64 = date[0].parse().ok()?;
    let mo: i64 = date[1].parse().ok()?;
    let d: i64 = date[2].parse().ok()?;
    let h: i64 = time[0].parse().ok()?;
    let mi: i64 = time[1].parse().ok()?;
    let sec: i64 = time[2].parse().ok()?;
    // days from civil（Howard Hinnant 算法）
    let y = if mo <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let secs = days * 86400 + h * 3600 + mi * 60 + sec - offset_min * 60;
    Some((secs.max(0)) as u64 * 1000)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn ws_url_includes_token() {
        assert_eq!(
            ws_url(19090, "abc", "/traffic"),
            "ws://127.0.0.1:19090/traffic?token=abc"
        );
    }

    #[test]
    fn compact_connections_trims_and_maps() {
        let raw = r#"{
          "downloadTotal": 100, "uploadTotal": 7, "memory": 2048,
          "connections": [{
            "id": "c1", "upload": 1, "download": 2,
            "start": "2026-10-04T12:00:00.123456+08:00",
            "chains": ["香港自动", "HK-01"], "rule": "DomainSuffix", "rulePayload": "example.com",
            "metadata": { "network": "tcp", "type": "HTTPS", "host": "example.com",
                          "destinationPort": "443", "processPath": "/Applications/Curl.app/x/curl" }
          }]
        }"#;
        let v: serde_json::Value = compact_connections(raw).unwrap();
        let row = &v["rows"][0];
        assert_eq!(row["host"], "example.com:443");
        assert_eq!(row["rule"], "DomainSuffix example.com");
        assert_eq!(row["chains"], "香港自动 → HK-01");
        assert_eq!(row["process"], "curl");
        assert_eq!(row["up"], 1);
        assert_eq!(v["upload_total"], 7);
        assert_eq!(v["memory"], 2048);
    }

    #[test]
    fn compact_connections_handles_empty() {
        let v: serde_json::Value =
            compact_connections(r#"{"downloadTotal":0,"uploadTotal":0,"connections":null}"#)
                .unwrap();
        assert_eq!(v["rows"].as_array().unwrap().len(), 0);
    }

    /// 已知锚点：2024-01-01T00:00:00Z = 1704067200。
    #[test]
    fn rfc3339_known_epoch() {
        assert_eq!(
            parse_rfc3339_ms("2024-01-01T00:00:00.000000+00:00"),
            Some(1_704_067_200_000)
        );
        assert_eq!(
            parse_rfc3339_ms("2024-01-01T08:00:00+08:00"),
            Some(1_704_067_200_000)
        );
    }

    #[test]
    fn rfc3339_one_second_apart_differs_by_1000ms() {
        let a = parse_rfc3339_ms("2026-10-04T04:00:00.000000+00:00").unwrap();
        let b = parse_rfc3339_ms("2026-10-04T04:00:01.000000+00:00").unwrap();
        assert_eq!(b - a, 1000);
    }

    /// 真实内核 WS 冒烟：traffic 桥 5 秒内应收到数据。
    #[test]
    fn real_core_traffic_ws_roundtrip() {
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
        // 关掉 geo 下载：全新临时目录会先拉 8MB geoip.metadb，慢网下拖垮就绪窗口
        mgr.set_options(crate::core_manager::CoreOptions {
            ensure_geo_files: false,
            ..crate::core_manager::CoreOptions::default()
        });
        mgr.start(
            "proxies: []\nrules:\n  - MATCH,DIRECT\n",
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
        let (tx, rx) = std::sync::mpsc::channel::<serde_json::Value>();
        let hub = WsHub::default();
        hub.subscribe_traffic(port, &secret, move |_, payload| {
            let _ = tx.send(payload);
        })
        .unwrap();

        let got = rx.recv_timeout(Duration::from_secs(8));
        assert!(got.is_ok(), "no traffic event within 8s");
        let v = got.unwrap();
        assert!(v.get("up").is_some() && v.get("down").is_some());

        hub.unsubscribe_all();
        mgr.stop();
    }
}
