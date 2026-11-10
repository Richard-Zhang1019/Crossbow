//! 内核 WS 数据桥：traffic / connections / logs 三路订阅。
//!
//! 设计要点（DESIGN §5.2）：连接按页面订退；最近日志由独立常驻读线程缓存，
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
pub const EV_LOG_STATUS: &str = "logs://status";

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
    /// channel 名 → (活跃标志, 最近一条消息时刻)。false = 已请求退订，读线程自行退出。
    /// 重订同Channel时旧标志被置 false（读线程最迟约 2 秒退出），新读线程顶替——
    /// 「订阅还在但已死/连着旧端口」的冻结形态由此自愈。
    active: Mutex<HashMap<&'static str, (Arc<AtomicBool>, Arc<Mutex<Instant>>)>>,
    /// 页面订阅意图集：挂载即登记、卸载即移除，与订阅是否成功无关。
    /// 内核重启后据此重订——覆盖「页面在内核 Starting 期间挂载、订阅
    /// 命令失败」的窗口，保证 Running 后自动补上。
    wanted: Arc<Mutex<std::collections::HashSet<&'static str>>>,
    logs: Arc<Mutex<crate::diagnostics::log_cache::LogCache>>,
    log_source: Mutex<Option<(u16, String)>>,
    log_view: Mutex<Option<String>>,
}

impl WsHub {
    pub fn begin_log_view(&self, subscriber: String) {
        let mut view = self.log_view.lock().unwrap_or_else(|e| e.into_inner());
        *view = Some(subscriber);
        self.want_channel("logs");
    }

    pub fn end_log_view(&self, subscriber: &str) {
        let mut view = self.log_view.lock().unwrap_or_else(|e| e.into_inner());
        if view.as_deref() == Some(subscriber) {
            *view = None;
            self.unsubscribe("logs");
        }
    }

    fn subscribe<F>(&self, name: &'static str, spawn_reader: F) -> Result<(), String>
    where
        F: FnOnce(Arc<AtomicBool>, Arc<Mutex<Instant>>) + Send + 'static,
    {
        let mut map = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((flag, _)) = map.get(name) {
            if flag.load(Ordering::SeqCst) {
                // 旧读线程仍在：请它退场（重订永远以最新 URL/回调为准）
                flag.store(false, Ordering::SeqCst);
            }
        }
        let flag = Arc::new(AtomicBool::new(true));
        let last_seen = Arc::new(Mutex::new(Instant::now()));
        map.insert(name, (flag.clone(), last_seen.clone()));
        drop(map);
        spawn_reader(flag, last_seen);
        Ok(())
    }

    /// 页面订阅意图登记：必须在订阅命令的最入口调用（require_controller
    /// 之前）——内核 Starting 期间挂载的页面订阅命令会失败，意图集是
    /// Running 后补订的唯一依据。
    pub fn want_channel(&self, name: &'static str) {
        self.wanted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(name);
    }

    /// 是否还有页面通道需要数据（有则内核 Running 后应重订）。
    pub fn any_page_channel_wanted(&self) -> bool {
        !self
            .wanted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    }

    /// 页面 1Hz 通道（traffic/connections）是否有「订阅在但消息停更」的
    /// 冻结——内核重启换 port/secret 后，旧读线程会在死端口上无限重连，
    /// 心跳停更即暴露。以 wanted 意图集为准（退订后 active 表项仍在，
    /// 不看意图会给没人要的通道反复重订）；logs 事件驱动、静默属正常，
    /// 不参与判定；心跳缺位（None）不判冻结：兼容 Starting 窗口的未订上状态。
    pub fn any_page_channel_stale(&self, threshold: Duration) -> bool {
        let wanted = self.wanted.lock().unwrap_or_else(|e| e.into_inner());
        ["traffic", "connections"].into_iter().any(|name| {
            wanted.contains(name)
                && self
                    .last_message_ago(name)
                    .is_some_and(|ago| ago >= threshold)
        })
    }

    /// 指定 channel 距最近一条消息的时长；从未订阅返回 None。
    pub fn last_message_ago(&self, name: &str) -> Option<Duration> {
        self.active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
            .map(|(_, t)| t.lock().unwrap_or_else(|e| e.into_inner()).elapsed())
    }

    pub fn unsubscribe(&self, name: &str) {
        self.wanted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(name);
        if let Some((flag, _)) = self
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
        {
            flag.store(false, Ordering::SeqCst);
        }
    }

    pub fn unsubscribe_all(&self) {
        for (flag, _) in self
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            flag.store(false, Ordering::SeqCst);
        }
    }

    /// 托盘实时速率专用订阅：独立 channel（与前端 traffic 互不干扰），
    /// 每次 tick 回调 (up, down) 字节/秒。菜单栏常驻，不受页面挂载影响。
    /// 带心跳追踪，供看门狗发现「订阅在但不再更新」的冻结并强制重订。
    pub fn subscribe_traffic_tray(
        &self,
        port: u16,
        secret: &str,
        on_tick: impl Fn(u64, u64) + Send + 'static,
    ) -> Result<(), String> {
        let url = ws_url(port, secret, "/traffic");
        self.subscribe("traffic-tray", move |flag, last_seen| {
            std::thread::spawn(move || {
                read_loop(
                    &url,
                    flag,
                    Some(Duration::from_secs(5)),
                    Some(last_seen),
                    None,
                    |msg| {
                        if let Some(Message::Text(txt)) = msg {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
                                let up = v.get("up").and_then(|x| x.as_u64()).unwrap_or(0);
                                let down = v.get("down").and_then(|x| x.as_u64()).unwrap_or(0);
                                on_tick(up, down);
                            }
                        }
                    },
                );
            });
        })
    }

    pub fn unsubscribe_tray_traffic(&self) {
        self.unsubscribe("traffic-tray");
    }

    pub fn subscribe_traffic(
        &self,
        port: u16,
        secret: &str,
        emit: impl Fn(&'static str, serde_json::Value) + Send + 'static,
    ) -> Result<(), String> {
        let url = ws_url(port, secret, "/traffic");
        self.subscribe("traffic", move |flag, last_seen| {
            std::thread::spawn(move || {
                read_loop(
                    &url,
                    flag,
                    Some(Duration::from_secs(5)),
                    Some(last_seen),
                    None,
                    |msg| {
                        if let Some(Message::Text(txt)) = msg {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
                                emit(EV_TRAFFIC, v);
                            }
                        }
                    },
                );
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
        self.subscribe("connections", move |flag, last_seen| {
            std::thread::spawn(move || {
                let mut last_emit = Instant::now() - CONN_MIN_INTERVAL;
                read_loop(
                    &url,
                    flag,
                    Some(Duration::from_secs(5)),
                    Some(last_seen),
                    None,
                    |msg| {
                        if let Some(Message::Text(txt)) = msg {
                            if last_emit.elapsed() < CONN_MIN_INTERVAL {
                                return;
                            }
                            if let Some(payload) = compact_connections(
                                &txt,
                                Some(&crate::platform::process_resolve::resolve_processes),
                            ) {
                                last_emit = Instant::now();
                                emit(EV_CONNECTIONS, payload);
                            }
                        }
                    },
                );
            });
        })
    }

    pub fn subscribe_logs(
        &self,
        port: u16,
        secret: &str,
        emit: impl Fn(&'static str, serde_json::Value) + Send + 'static,
    ) -> Result<(), String> {
        // A single process-lifetime reader serves page history and live events.
        // Page navigation must not restart it or lose logs generated off-page.
        let mut source = self.log_source.lock().unwrap_or_else(|e| e.into_inner());
        if source
            .as_ref()
            .is_some_and(|(p, s)| *p == port && s == secret)
            && self
                .active
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get("logs-cache")
                .is_some_and(|(flag, _)| flag.load(Ordering::SeqCst))
        {
            return Ok(());
        }
        *source = Some((port, secret.to_owned()));
        let cache = self.logs.clone();
        let wanted = self.wanted.clone();
        let generation = cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .begin_stream();
        let url = ws_url(port, secret, "/logs?level=info");
        self.subscribe("logs-cache", move |flag, _last_seen| {
            std::thread::spawn(move || {
                let mut batch: Vec<crate::diagnostics::log_cache::Entry> = Vec::new();
                let mut last_flush = Instant::now();
                let mut on_status = |connected| {
                    let status = cache.lock().unwrap_or_else(|e| e.into_inner()).set_connected(generation, connected);
                    if let Some(status) = status {
                        if wanted.lock().unwrap_or_else(|e| e.into_inner()).contains("logs") {
                            emit(EV_LOG_STATUS, serde_json::json!({ "connected": status.connected, "generation": status.generation }));
                            // Drain cached entries missed just before reconnect;
                            // client sequence IDs remove any overlap with live batches.
                            if connected {
                                let snapshot = cache.lock().unwrap_or_else(|e| e.into_inner()).snapshot();
                                if !snapshot.entries.is_empty() { emit(EV_LOGS, serde_json::json!(snapshot.entries)); }
                            }
                        }
                    }
                };
                read_loop(&url, flag, None, None, Some(&mut on_status), |msg| {
                    if let Some(Message::Text(txt)) = msg {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
                            if let Some(entry) = cache.lock().unwrap_or_else(|e| e.into_inner()).append(generation, &v) {
                                batch.push(entry);
                            }
                        }
                    }
                    // Flush even when the core sends just one line, then goes
                    // quiet. Previously the next message was required to flush.
                    if !batch.is_empty()
                        && (last_flush.elapsed() >= LOG_FLUSH_INTERVAL
                            || batch.len() >= LOG_FLUSH_COUNT)
                    {
                        let entries = std::mem::take(&mut batch);
                        if wanted.lock().unwrap_or_else(|e| e.into_inner()).contains("logs") {
                            emit(EV_LOGS, serde_json::json!(entries));
                        }
                        last_flush = Instant::now();
                    }
                });
                if !batch.is_empty() && wanted.lock().unwrap_or_else(|e| e.into_inner()).contains("logs") {
                    emit(EV_LOGS, serde_json::json!(batch));
                }
            });
        })
    }

    pub fn logs_snapshot(&self) -> crate::diagnostics::log_cache::Snapshot {
        self.logs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .snapshot()
    }

    pub fn clear_logs(&self) -> crate::diagnostics::log_cache::Snapshot {
        self.logs.lock().unwrap_or_else(|e| e.into_inner()).clear()
    }

    pub fn stop_logs(&self) {
        let mut source = self.log_source.lock().unwrap_or_else(|e| e.into_inner());
        *source = None;
        self.unsubscribe("logs-cache");
        // Invalidate callbacks still draining the old socket, retaining history.
        self.logs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .begin_stream();
    }

    /// 日志落盘专用订阅（#29）：独立常驻通道，不随日志页挂载/卸载——
    /// 只要内核在跑就持续把 info 级日志交给落盘端（level/type/payload）。
    /// sink 持有 DailyWriter，内部自缓冲。
    pub fn subscribe_logs_file(
        &self,
        port: u16,
        secret: &str,
        mut sink: impl FnMut(&str, &str) + Send + 'static,
    ) -> Result<(), String> {
        let url = ws_url(port, secret, "/logs?level=info");
        self.subscribe("logs-file", move |flag, _last_seen| {
            std::thread::spawn(move || {
                read_loop(&url, flag, None, None, None, |msg| {
                    if let Some(Message::Text(txt)) = msg {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
                            let level = v.get("type").and_then(|x| x.as_str()).unwrap_or("info");
                            let payload = v.get("payload").and_then(|x| x.as_str()).unwrap_or("");
                            if !payload.is_empty() {
                                sink(level, payload);
                            }
                        }
                    }
                });
            });
        })
    }
}

fn ws_url(port: u16, secret: &str, path: &str) -> String {
    // path 可能自带查询串（如 /logs?level=info）——token 必须用 & 续接，
    // 否则 token 被吞进上一个参数值，内核 401，日志通道无限重连。
    let sep = if path.contains('?') { "&" } else { "?" };
    format!("ws://127.0.0.1:{port}{path}{sep}token={secret}")
}

/// 阻塞式 WS 读循环：断线自动重连，退订后于下一条消息前退出。
/// WS 读循环（自动重连）。
///
/// `stale`：静默判定窗口。给到值时给底层 TCP 设读超时，超过 `stale`
/// 未收到任何帧即断开重连——修复 TCP 半开（睡眠唤醒/网络切换/内核假死）
/// 导致 read 永久阻塞、托盘速率冻结在旧值的问题。None = 保持旧行为
/// （适合长期无推送也属正常的通道，如日志）。
///
/// `last_seen`：每收到一条文本帧即刷新，供上层看门狗判断通道活性
/// （读线程意外死亡时心跳停走，看门狗据此强制重订）。
fn read_loop(
    url: &str,
    flag: Arc<AtomicBool>,
    stale: Option<Duration>,
    last_seen: Option<Arc<Mutex<Instant>>>,
    mut on_status: Option<&mut dyn FnMut(bool)>,
    mut on_text: impl FnMut(Option<Message>),
) {
    const READ_TIMEOUT: Duration = Duration::from_secs(2);
    'outer: while flag.load(Ordering::SeqCst) {
        let mut ws = match tungstenite::connect(url) {
            Ok((ws, _)) => ws,
            Err(_) => {
                if flag.load(Ordering::SeqCst) {
                    if let Some(status) = on_status.as_mut() {
                        status(false);
                    }
                }
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        if !flag.load(Ordering::SeqCst) {
            break;
        }
        if let Some(status) = on_status.as_mut() {
            status(true);
        }
        if let tungstenite::stream::MaybeTlsStream::Plain(tcp) = ws.get_ref() {
            let timeout = if stale.is_some() {
                READ_TIMEOUT
            } else {
                LOG_FLUSH_INTERVAL
            };
            let _ = tcp.set_read_timeout(Some(timeout));
        }
        let mut last_msg = Instant::now();
        loop {
            if !flag.load(Ordering::SeqCst) {
                break 'outer;
            }
            match ws.read() {
                Ok(Message::Ping(_) | Message::Pong(_)) => {}
                Ok(msg @ Message::Text(_)) => {
                    last_msg = Instant::now();
                    if let Some(t) = &last_seen {
                        *t.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
                    }
                    on_text(Some(msg));
                }
                Ok(Message::Close(_)) => break, // 内核重启中，走重连
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    // 读超时≠断线：仅在静默超过 stale 窗口时才判定半开重连
                    on_text(None);
                    if stale.is_some_and(|limit| last_msg.elapsed() > limit) {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        if flag.load(Ordering::SeqCst) {
            if let Some(status) = on_status.as_mut() {
                status(false);
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
    process_path: String,
    elapsed_ms: Option<u64>,
}

#[derive(Serialize)]
struct ConnPayload {
    rows: Vec<ConnRow>,
    upload_total: Option<u64>,
    download_total: Option<u64>,
    memory: Option<u64>,
    /// 超过 CONN_MAX_ROWS 被截断时的原始总行数。
    #[serde(skip_serializing_if = "Option::is_none")]
    truncated: Option<usize>,
}

/// mihomo connections 快照 → 紧凑结构（字段裁剪让大快照的 IPC 载荷可控）。
/// `process_fallback`：mihomo 进程识别缺失时按源端口批量补齐（#418，macOS
/// 全失效 / Windows UDP 竞态）；传 None 跳过补齐（测试路径）。
fn compact_connections(
    raw: &str,
    process_fallback: Option<&dyn Fn(&[u16]) -> std::collections::HashMap<u16, String>>,
) -> Option<serde_json::Value> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let now_ms = now_unix_ms();
    let mut rows = Vec::new();
    // 先收集需要补齐的源端口（进程为空且非内核自身出站），一次批量反查。
    let mut missing_ports: Vec<u16> = Vec::new();
    if process_fallback.is_some() {
        if let Some(conns) = v.get("connections").and_then(|c| c.as_array()) {
            for c in conns {
                let md = c.get("metadata").cloned().unwrap_or_default();
                let str_at = |k: &str| md.get(k).and_then(|x| x.as_str()).unwrap_or("");
                if !str_at("processPath").is_empty() {
                    continue;
                }
                // 源端口 0（TUN 场景无本机 socket）没有反查意义。
                if let Ok(port) = str_at("sourcePort").parse::<u16>() {
                    if port != 0 {
                        missing_ports.push(port);
                    }
                }
            }
        }
    }
    let resolved = process_fallback
        .map(|f| f(&missing_ports))
        .unwrap_or_default();
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
            // 进程补齐：内核没给时用反查表；反查命中的进程可能就是 mihomo
            // 自身（内核自身出站连接），这正好如实呈现，无需排除。
            let process_path = if str_at("processPath").is_empty() {
                str_at("sourcePort")
                    .parse::<u16>()
                    .ok()
                    .and_then(|p| resolved.get(&p).cloned())
                    .unwrap_or_default()
            } else {
                str_at("processPath").to_string()
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
                process: process_path
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or("")
                    .to_string(),
                process_path,
                elapsed_ms: c
                    .get("start")
                    .and_then(|x| x.as_str())
                    .and_then(parse_rfc3339_ms)
                    .and_then(|start| now_ms.checked_sub(start)),
            });
        }
    }
    let total_rows = rows.len();
    rows.truncate(CONN_MAX_ROWS);
    let payload = ConnPayload {
        upload_total: v.get("uploadTotal").and_then(|x| x.as_u64()),
        download_total: v.get("downloadTotal").and_then(|x| x.as_u64()),
        memory: v.get("memory").and_then(|x| x.as_u64()),
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

/// Parse RFC3339 without dropping the timezone after fractional seconds.
fn parse_rfc3339_ms(s: &str) -> Option<u64> {
    if !s.is_ascii() || s.len() < 20 {
        return None;
    }
    if &s[4..5] != "-"
        || &s[7..8] != "-"
        || !matches!(&s[10..11], "T" | "t")
        || &s[13..14] != ":"
        || &s[16..17] != ":"
    {
        return None;
    }
    let number = |a: usize, b: usize| -> Option<i64> {
        let part = s.get(a..b)?;
        if !part.bytes().all(|v| v.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    };
    let year = number(0, 4)?;
    let mo = number(5, 7)?;
    let day = number(8, 10)?;
    let hour = number(11, 13)?;
    let minute = number(14, 16)?;
    let second = number(17, 19)?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in_month = match mo {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        _ => return None,
    };
    if !(1..=days_in_month).contains(&day) || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let mut suffix = &s[19..];
    let mut millis = 0i64;
    if let Some(fraction) = suffix.strip_prefix('.') {
        let digits = fraction.bytes().take_while(|b| b.is_ascii_digit()).count();
        if digits == 0 {
            return None;
        }
        let first = &fraction[..digits.min(3)];
        millis = first.parse::<i64>().ok()? * 10i64.pow(3 - first.len() as u32);
        suffix = &fraction[digits..];
    }
    let offset = if suffix == "Z" || suffix == "z" {
        0
    } else {
        if suffix.len() != 6 || !matches!(&suffix[..1], "+" | "-") || &suffix[3..4] != ":" {
            return None;
        }
        if !suffix[1..3]
            .bytes()
            .chain(suffix[4..6].bytes())
            .all(|b| b.is_ascii_digit())
        {
            return None;
        }
        let h = suffix[1..3].parse::<i64>().ok()?;
        let m = suffix[4..6].parse::<i64>().ok()?;
        if h > 23 || m > 59 {
            return None;
        }
        (h * 60 + m) * if suffix.starts_with('-') { -1 } else { 1 }
    };
    let y = year - if mo <= 2 { 1 } else { 0 };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = mo + if mo > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let days = era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468;
    let timestamp =
        (days * 86400 + hour * 3600 + minute * 60 + second - offset * 60) * 1000 + millis;
    u64::try_from(timestamp).ok()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn connection_times_keep_fractional_timezone_and_reject_invalid_dates() {
        let utc = parse_rfc3339_ms("2026-11-03T04:00:00.123Z").unwrap();
        assert_eq!(
            parse_rfc3339_ms("2026-11-03T12:00:00.123456+08:00"),
            Some(utc)
        );
        assert_eq!(parse_rfc3339_ms("2026-11-02T23:00:00.123-05:00"), Some(utc));
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339_ms("2024-02-29T00:00:00Z"),
            Some(1709164800000)
        );
        for bad in [
            "",
            "broken",
            "2025-02-29T00:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-11-03T25:00:00Z",
            "2026-11-03T00:00:00",
            "2026-11-03T00:00:00.Z",
        ] {
            assert_eq!(parse_rfc3339_ms(bad), None, "{bad}");
        }
    }

    #[test]
    fn missing_connection_metrics_stay_null_and_zero_stays_zero() {
        let data = compact_connections(
            r#"{"connections":[{"id":"missing"},{"id":"future","start":"9999-01-01T00:00:00Z"}],"memory":0}"#,
            None,
        )
        .unwrap();
        assert_eq!(data["memory"], 0);
        assert!(data["upload_total"].is_null());
        assert!(data["rows"][0]["elapsed_ms"].is_null());
        assert!(data["rows"][1]["elapsed_ms"].is_null());
        assert!(compact_connections(r#"{"connections":[]}"#, None).unwrap()["memory"].is_null());
    }

    #[test]
    fn single_log_is_flushed_while_stream_is_quiet_and_unsubscribes() {
        use std::net::TcpListener;
        use std::sync::mpsc;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(4)))
                .unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            socket
                .send(Message::Text(
                    r#"{"type":"info","payload":"single log"}"#.into(),
                ))
                .unwrap();
            // No second frame: delivery must be driven by the idle timeout.
            let _ = socket.read();
        });
        let hub = WsHub::default();
        let (send, receive) = mpsc::channel();
        hub.begin_log_view("test-page".into());
        hub.subscribe_logs(port, "test-secret", move |event, payload| {
            let _ = send.send((event, payload));
        })
        .unwrap();
        let connected = receive.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(connected.0, EV_LOG_STATUS);
        assert_eq!(connected.1["connected"], true);
        let log = receive.recv_timeout(Duration::from_secs(2));
        hub.end_log_view("test-page");
        assert!(!hub.logs_snapshot().entries.is_empty());
        hub.stop_logs();
        server.join().unwrap();
        let (event, payload) = log.expect("a single log must not wait for the next message");
        assert_eq!(event, EV_LOGS);
        assert_eq!(payload[0]["payload"], "single log");
    }

    #[test]
    fn background_logs_survive_page_navigation_without_reconnecting() {
        use std::net::TcpListener;
        use std::sync::mpsc;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (send_log, log_commands) = mpsc::channel::<&str>();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            while let Ok(payload) = log_commands.recv_timeout(Duration::from_secs(5)) {
                socket
                    .send(Message::Text(
                        serde_json::json!({"type":"info","payload":payload})
                            .to_string()
                            .into(),
                    ))
                    .unwrap();
            }
        });
        let hub = WsHub::default();
        let (events, receive) = mpsc::channel();
        hub.subscribe_logs(port, "test-secret", move |event, data| {
            let _ = events.send((event, data));
        })
        .unwrap();
        send_log.send("off-page").unwrap();
        let wait_for_logs = |count| {
            let deadline = Instant::now() + Duration::from_secs(3);
            while hub.logs_snapshot().entries.len() < count {
                assert!(Instant::now() < deadline, "cache did not receive log");
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        wait_for_logs(1);
        assert_eq!(hub.logs_snapshot().entries[0].payload, "off-page");
        assert!(
            receive.try_recv().is_err(),
            "background collection must not emit to absent page"
        );
        let reader = hub
            .active
            .lock()
            .unwrap()
            .get("logs-cache")
            .unwrap()
            .0
            .clone();
        hub.begin_log_view("old".into());
        hub.begin_log_view("new".into());
        hub.end_log_view("old"); // A late cleanup must not unsubscribe the new view.
        assert!(hub.any_page_channel_wanted());
        hub.subscribe_logs(port, "test-secret", |_, _| {
            panic!("must retain original reader")
        })
        .unwrap();
        assert!(Arc::ptr_eq(
            &reader,
            &hub.active.lock().unwrap().get("logs-cache").unwrap().0
        ));
        send_log.send("on-page").unwrap();
        wait_for_logs(2);
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let (event, entries) = receive
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap();
            if event == EV_LOGS
                && entries
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|entry| entry["payload"] == "on-page")
            {
                break;
            }
        }
        hub.end_log_view("new");
        send_log.send("after-leaving").unwrap();
        wait_for_logs(3);
        assert_eq!(hub.logs_snapshot().entries[2].payload, "after-leaving");
        hub.stop_logs();
        assert!(!hub.logs_snapshot().connected);
        assert_eq!(hub.logs_snapshot().entries.len(), 3);
        drop(send_log);
        server.join().unwrap();
    }

    #[test]
    fn ws_url_includes_token() {
        assert_eq!(
            ws_url(19090, "abc", "/traffic"),
            "ws://127.0.0.1:19090/traffic?token=abc"
        );
        // 自带查询串的 path 用 & 续接 token（日志通道 401 死循环回归测试）
        assert_eq!(
            ws_url(19090, "abc", "/logs?level=info"),
            "ws://127.0.0.1:19090/logs?level=info&token=abc"
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
        let v: serde_json::Value = compact_connections(raw, None).unwrap();
        let row = &v["rows"][0];
        assert_eq!(row["host"], "example.com:443");
        assert_eq!(row["rule"], "DomainSuffix example.com");
        assert_eq!(row["chains"], "香港自动 → HK-01");
        assert_eq!(row["process"], "curl");
        assert_eq!(row["up"], 1);
        assert_eq!(v["upload_total"], 7);
        assert_eq!(v["memory"], 2048);
    }

    /// #418：内核进程元数据缺失时按源端口走 fallback 反查；命中与未命中
    /// 各留原始形态（命中 → 进程名，未命中 → 空串）。
    #[test]
    fn compact_connections_falls_back_to_port_resolver() {
        let raw = r#"{
          "connections": [{
            "id": "c1", "metadata": { "network": "tcp", "type": "HTTPS",
                          "host": "example.com", "destinationPort": "443",
                          "sourcePort": "59478" }
          },{
            "id": "c2", "metadata": { "network": "tcp", "type": "HTTPS",
                          "host": "unknown.test", "destinationPort": "443",
                          "sourcePort": "1234" }
          },{
            "id": "c3", "metadata": { "network": "tcp", "type": "HTTPS",
                          "host": "hasproc.test", "destinationPort": "443",
                          "processPath": "/Applications/Safari.app/Safari" }
          }]
        }"#;
        let resolved = |ports: &[u16]| {
            assert_eq!(ports, &[59478, 1234], "应只批量反查缺失且端口非零的连接");
            let mut m = std::collections::HashMap::new();
            m.insert(59478u16, "Safari".to_string());
            m
        };
        let v: serde_json::Value = compact_connections(raw, Some(&resolved)).unwrap();
        let rows = v["rows"].as_array().unwrap();
        assert_eq!(rows[0]["process"], "Safari");
        assert_eq!(rows[1]["process"], "");
        assert_eq!(rows[2]["process"], "Safari");
    }

    /// TUN 场景 sourcePort=0 无本机 socket，不应进入反查列表。
    #[test]
    fn compact_connections_skips_zero_source_port() {
        let raw = r#"{
          "connections": [{
            "id": "tun", "metadata": { "network": "tcp", "type": "HTTPS",
                          "host": "tun.test", "destinationPort": "443",
                          "sourcePort": "0" }
          }]
        }"#;
        let resolved = |ports: &[u16]| {
            assert!(ports.is_empty(), "零端口不应参与反查");
            std::collections::HashMap::new()
        };
        let v: serde_json::Value = compact_connections(raw, Some(&resolved)).unwrap();
        assert_eq!(v["rows"][0]["process"], "");
    }

    #[test]
    fn compact_connections_handles_empty() {
        let v: serde_json::Value = compact_connections(
            r#"{"downloadTotal":0,"uploadTotal":0,"connections":null}"#,
            None,
        )
        .unwrap();
        assert_eq!(v["rows"].as_array().unwrap().len(), 0);
    }

    /// 页面通道心跳与冻结判定：订阅即有心跳；阈值内不算冻结、归零即冻结；
    /// logs 不参与判定（事件驱动，静默属正常）；退订（wanted 移除）后
    /// 不再参与判定。#111
    #[test]
    fn page_channel_heartbeat_and_staleness() {
        let hub = WsHub::default();
        // 心跳缺位（未订阅）不判冻结
        assert!(!hub.any_page_channel_stale(Duration::from_secs(15)));
        // 生产入口（lib.rs 订阅命令）先登记意图，再走桥订阅；连不上的
        // 死端口也要注册心跳（active 表项同步插入）
        hub.want_channel("traffic");
        hub.subscribe_traffic(1, "s", |_, _| {}).unwrap();
        assert!(hub.last_message_ago("traffic").is_some());
        assert!(!hub.any_page_channel_stale(Duration::from_secs(15)));
        assert!(hub.any_page_channel_stale(Duration::from_secs(0)));
        // logs 不参与 1Hz 冻结判定：仅 traffic 冻结仍判冻结
        hub.subscribe_logs(1, "s", |_, _| {}).unwrap();
        assert!(hub.any_page_channel_stale(Duration::from_secs(0)));
        // traffic 退订（wanted 移除）后只剩不参与判定的 logs：不判冻结
        hub.unsubscribe("traffic");
        assert!(!hub.any_page_channel_stale(Duration::from_secs(0)));
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
        let _serial = crate::engine::manager::tests::REAL_CORE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Ok(bin) = std::env::var("CROSSBOW_CORE_BIN") else {
            eprintln!("skip: CROSSBOW_CORE_BIN not set");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let mgr = crate::engine::manager::CoreManager::new(
            std::path::PathBuf::from(bin),
            dir.path().to_path_buf(),
        );
        // 关掉 geo 下载：全新临时目录会先拉 8MB geoip.metadb，慢网下拖垮就绪窗口
        mgr.set_options(crate::engine::manager::CoreOptions {
            ensure_geo_files: false,
            ..crate::engine::manager::CoreOptions::default()
        });
        mgr.start(
            "proxies: []\nrules:\n  - MATCH,DIRECT\n",
            &crossbow_core::RuntimeConfig {
                mixed_port: crate::engine::manager::tests::tests_free_port(),
                ..crossbow_core::RuntimeConfig::default()
            },
        )
        .unwrap();
        let mut running = false;
        for _ in 0..100 {
            if mgr.status() == crate::engine::manager::CoreStatus::Running {
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
