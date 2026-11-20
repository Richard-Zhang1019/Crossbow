//! #567 本地流量统计：按域名/出口节点的日累计聚合与 TopN 查询。
//!
//! 数据源：mihomo /connections WS 帧——常驻低频采样（约 5s 解析一帧，
//! 与页面连接订阅互不相干）；帧内 upload/download 是连接累计值，这里
//! 维护上一帧基准算增量，连接从帧中消失即视为关闭、基准随手清理。
//! 存储：数据目录 traffic-stats.json，日桶按**本地日期**切割（跨天 =
//! 换 key），节流落盘（≥30s 且有增量），启动加载，保留窗口默认 30 天。
//!
//! 性能边界：跳帧不丢增量（累计值语义），高连接数下解析频率固定、
//! 单帧成本线性于连接数且只做哈希累加；落盘节流，UI 只读聚合结果。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// 统计保留窗口（本地日）。窗口外的桶在落盘前裁剪。
pub(crate) const RETENTION_DAYS: u32 = 30;
/// 采样解析最小间隔：mihomo 帧为 1Hz 全量连接列表，累计值语义下跳帧
/// 不丢增量，5s 一帧显著压低高连接数下的解析开销。
pub(crate) const STATS_PARSE_MIN_INTERVAL: Duration = Duration::from_secs(5);
/// 落盘最小间隔（有增量才写）。
const FLUSH_MIN_INTERVAL: Duration = Duration::from_secs(30);
/// TopN 条数。
const TOPN: usize = 10;

/// 单条连接的上一帧累计基准。
#[derive(Default)]
struct ConnBasis {
    up: u64,
    down: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct DayBuckets {
    /// host → [up, down]
    domains: HashMap<String, [u64; 2]>,
    /// 出口节点 → [up, down]
    nodes: HashMap<String, [u64; 2]>,
}

#[derive(Default, Serialize, Deserialize)]
struct StatsFile {
    /// 本地日期（YYYY-MM-DD，ISO 排序即时间序）→ 桶。
    days: BTreeMap<String, DayBuckets>,
}

#[derive(Serialize)]
struct StatsFileRef<'a> {
    days: &'a BTreeMap<String, DayBuckets>,
}

/// 一帧 /connections 里参与聚合的最小字段集。
pub(crate) struct ConnItem {
    pub(crate) id: String,
    pub(crate) up: u64,
    pub(crate) down: u64,
    pub(crate) host: String,
    pub(crate) node: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct TrafficStatRow {
    pub(crate) key: String,
    pub(crate) up: u64,
    pub(crate) down: u64,
}

#[derive(Debug, Serialize)]
pub(crate) struct TrafficStatsReport {
    pub(crate) domains: Vec<TrafficStatRow>,
    pub(crate) nodes: Vec<TrafficStatRow>,
    /// 窗口内逐日总量（key = 日期，升序），供每日总量图表。
    pub(crate) daily: Vec<TrafficStatRow>,
}

pub(crate) struct TrafficStats {
    path: PathBuf,
    inner: Mutex<StatsInner>,
}

struct StatsInner {
    days: BTreeMap<String, DayBuckets>,
    basis: HashMap<String, ConnBasis>,
    dirty: bool,
    last_flush: Option<Instant>,
}

/// 解析 mihomo /connections 帧的参与字段。host 缺失回退 destinationIP
/// （归属原始地址）；node 取 chains[0]（实际出口，与连接页口径一致）。
pub(crate) fn parse_frame(txt: &str) -> Option<Vec<ConnItem>> {
    let value: serde_json::Value = serde_json::from_str(txt).ok()?;
    let array = value.get("connections")?.as_array()?;
    let mut items = Vec::with_capacity(array.len());
    for item in array {
        let id = item.get("id")?.as_str()?.to_string();
        let up = item.get("upload").and_then(|x| x.as_u64()).unwrap_or(0);
        let down = item.get("download").and_then(|x| x.as_u64()).unwrap_or(0);
        let metadata = item.get("metadata");
        let host = metadata
            .and_then(|m| m.get("host"))
            .and_then(|h| h.as_str())
            .filter(|h| !h.is_empty())
            .or_else(|| {
                metadata
                    .and_then(|m| m.get("destinationIP"))
                    .and_then(|h| h.as_str())
                    .filter(|h| !h.is_empty())
            })
            .unwrap_or("unknown")
            .to_string();
        let node = item
            .get("chains")
            .and_then(|c| c.as_array())
            .and_then(|c| c.first())
            .and_then(|n| n.as_str())
            .unwrap_or("unknown")
            .to_string();
        items.push(ConnItem {
            id,
            up,
            down,
            host,
            node,
        });
    }
    Some(items)
}

/// ISO 日期串按字典序即时间序：窗口起点 = today − (days−1) 天。
fn window_start(today: &str, days: u32) -> String {
    chrono::NaiveDate::parse_from_str(today, "%Y-%m-%d")
        .map(|d| {
            d.checked_sub_days(chrono::Days::new(days.saturating_sub(1) as u64))
                .unwrap_or(d)
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_default()
}

fn local_today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

impl TrafficStats {
    pub(crate) fn open(data_dir: &Path) -> Self {
        let path = data_dir.join("traffic-stats.json");
        let days = match fs::read(&path) {
            Ok(bytes) => {
                serde_json::from_slice::<StatsFile>(&bytes)
                    .unwrap_or_default()
                    .days
            }
            // 首次/损坏都从空表开始：统计是锦上添花，不能挡启动
            Err(_) => BTreeMap::new(),
        };
        Self {
            path,
            inner: Mutex::new(StatsInner {
                days,
                basis: HashMap::new(),
                dirty: false,
                last_flush: None,
            }),
        }
    }

    /// 聚合一帧：增量 = 当前累计 − 上一帧基准（saturating，计数器回退
    /// 不产生负增量）；帧中消失的连接视为关闭，基准随手清理。
    /// `day` 为本地日期，跨天切割即换 key。
    pub(crate) fn apply_frame(&self, items: &[ConnItem], day: &str) {
        // 显式 &mut 拆字段借用：days 与 basis 的 entry 同时活跃（MutexGuard
        // 的 DerefMut 不会做字段级 borrow split）
        let inner: &mut StatsInner = &mut self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let seen: HashSet<&str> = items.iter().map(|item| item.id.as_str()).collect();
        let mut changed = false;
        let day_entry = inner.days.entry(day.to_string()).or_default();
        for item in items {
            let basis = inner.basis.entry(item.id.clone()).or_default();
            let up = item.up.saturating_sub(basis.up);
            let down = item.down.saturating_sub(basis.down);
            basis.up = item.up;
            basis.down = item.down;
            if up == 0 && down == 0 {
                continue;
            }
            changed = true;
            let d = day_entry.domains.entry(item.host.clone()).or_insert([0, 0]);
            d[0] += up;
            d[1] += down;
            let n = day_entry.nodes.entry(item.node.clone()).or_insert([0, 0]);
            n[0] += up;
            n[1] += down;
        }
        inner.basis.retain(|id, _| seen.contains(id.as_str()));
        if changed {
            inner.dirty = true;
        }
    }

    /// 常驻采样入口：解析一帧 → 聚合 → 按需落盘。
    pub(crate) fn ingest(&self, txt: &str) {
        let Some(items) = parse_frame(txt) else {
            return;
        };
        self.apply_frame(&items, &local_today());
        self.flush_if_due(false);
    }

    /// 节流落盘：有增量且距上次 ≥30s 才写；`force` 供核心停止/退订时
    /// 跳过时间闸。写入前按保留窗口裁剪。
    pub(crate) fn flush_if_due(&self, force: bool) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if !inner.dirty {
            return;
        }
        if !force
            && inner
                .last_flush
                .is_some_and(|t| t.elapsed() < FLUSH_MIN_INTERVAL)
        {
            return;
        }
        // 保留窗口锚定数据中最新 day（BTreeMap 有序，恒为业务上的今天），
        // 而非真实系统时钟：写入方（ingest 用 local_today）正常时两者一致；
        // 时钟回拨/时区差或测试注入固定日期时，锚真实时钟会把「数据里仍在
        // 窗口内语义的条目」裁掉（#581 CI 实挂：UTC 日期晚于测试固定日期
        // 一天，窗口错位导致 32 天 > 30）。
        if let Some(latest) = inner.days.keys().next_back() {
            let cutoff = window_start(latest, RETENTION_DAYS);
            if !cutoff.is_empty() {
                inner.days.retain(|day, _| day.as_str() >= cutoff.as_str());
            }
        }
        let payload = serde_json::to_vec(&StatsFileRef { days: &inner.days }).unwrap_or_default();
        // 写失败保持 dirty 下轮再试；统计不应打扰用户
        if fs::write(&self.path, payload).is_ok() {
            inner.dirty = false;
        }
        inner.last_flush = Some(Instant::now());
    }

    /// 窗口内（含今天）TopN + 逐日总量。`days=1` 即仅今天。
    pub(crate) fn topn(&self, days: u32) -> TrafficStatsReport {
        self.topn_with_today(days, &local_today())
    }

    pub(crate) fn topn_with_today(&self, days: u32, today: &str) -> TrafficStatsReport {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let start = window_start(today, days.max(1));
        let mut domains: HashMap<&str, [u64; 2]> = HashMap::new();
        let mut nodes: HashMap<&str, [u64; 2]> = HashMap::new();
        let mut daily: Vec<TrafficStatRow> = Vec::new();
        for (day, buckets) in inner.days.range(start..=today.to_string()) {
            let mut day_up = 0u64;
            let mut day_down = 0u64;
            for (key, bytes) in &buckets.domains {
                let entry = domains.entry(key.as_str()).or_insert([0, 0]);
                entry[0] += bytes[0];
                entry[1] += bytes[1];
                day_up += bytes[0];
                day_down += bytes[1];
            }
            for (key, bytes) in &buckets.nodes {
                let entry = nodes.entry(key.as_str()).or_insert([0, 0]);
                entry[0] += bytes[0];
                entry[1] += bytes[1];
            }
            daily.push(TrafficStatRow {
                key: day.clone(),
                up: day_up,
                down: day_down,
            });
        }
        TrafficStatsReport {
            domains: rank(domains),
            nodes: rank(nodes),
            daily,
        }
    }
}

/// 按总字节降序取前 TOPN。
fn rank(map: HashMap<&str, [u64; 2]>) -> Vec<TrafficStatRow> {
    let mut rows: Vec<TrafficStatRow> = map
        .into_iter()
        .map(|(key, [up, down])| TrafficStatRow {
            key: key.to_string(),
            up,
            down,
        })
        .collect();
    rows.sort_by(|a, b| {
        (b.up + b.down)
            .cmp(&(a.up + a.down))
            .then_with(|| a.key.cmp(&b.key))
    });
    rows.truncate(TOPN);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, up: u64, down: u64, host: &str, node: &str) -> ConnItem {
        ConnItem {
            id: id.to_string(),
            up,
            down,
            host: host.to_string(),
            node: node.to_string(),
        }
    }

    fn open_temp() -> (TrafficStats, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (TrafficStats::open(dir.path()), dir)
    }

    #[test]
    fn parse_frame_reads_mihomo_shape() {
        let txt = r#"{
            "downloadTotal": 1, "uploadTotal": 2, "memory": 3,
            "connections": [
                {"id": "c1", "upload": 10, "download": 20,
                 "chains": ["HK-01", "规则组"],
                 "metadata": {"host": "example.com", "destinationIP": "1.2.3.4"}},
                {"id": "c2", "upload": 5, "download": 6,
                 "chains": ["DIRECT"],
                 "metadata": {"host": "", "destinationIP": "9.9.9.9"}}
            ]
        }"#;
        let items = parse_frame(txt).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].host, "example.com");
        assert_eq!(items[0].node, "HK-01");
        // host 缺失回退 destinationIP
        assert_eq!(items[1].host, "9.9.9.9");
        assert_eq!(items[1].node, "DIRECT");
    }

    #[test]
    fn parse_frame_survives_garbage_and_empty() {
        assert!(parse_frame("not json").is_none());
        assert!(parse_frame("{}").is_none());
        assert_eq!(parse_frame(r#"{"connections": []}"#).unwrap().len(), 0);
    }

    #[test]
    fn apply_frame_computes_cumulative_deltas() {
        let (stats, _dir) = open_temp();
        let day = "2026-11-20";
        // 新连接：首次全量计入
        stats.apply_frame(&[item("c1", 100, 200, "a.com", "HK-01")], day);
        let report = stats.topn_with_today(1, day);
        assert_eq!(report.domains[0].up, 100);
        assert_eq!(report.domains[0].down, 200);
        // 同连接增长：只计增量
        stats.apply_frame(&[item("c1", 150, 260, "a.com", "HK-01")], day);
        let report = stats.topn_with_today(1, day);
        assert_eq!(report.domains[0].up, 150);
        assert_eq!(report.domains[0].down, 260);
        // 计数器回退（理论不应发生）：不产生负增量
        stats.apply_frame(&[item("c1", 10, 20, "a.com", "HK-01")], day);
        let report = stats.topn_with_today(1, day);
        assert_eq!(report.domains[0].up, 150);
        assert_eq!(report.domains[0].down, 260);
    }

    #[test]
    fn apply_frame_drops_closed_connections() {
        let (stats, _dir) = open_temp();
        let day = "2026-11-20";
        stats.apply_frame(
            &[
                item("c1", 100, 0, "a.com", "HK-01"),
                item("c2", 50, 0, "b.com", "HK-01"),
            ],
            day,
        );
        // c2 关闭（帧中消失），c1 继续增长
        stats.apply_frame(&[item("c1", 200, 0, "a.com", "HK-01")], day);
        let report = stats.topn_with_today(1, day);
        let b = report.domains.iter().find(|r| r.key == "b.com").unwrap();
        assert_eq!(b.up, 50);
        // c2 重现：基准已清，按新连接全量计入
        stats.apply_frame(
            &[
                item("c1", 210, 0, "a.com", "HK-01"),
                item("c2", 70, 0, "b.com", "HK-01"),
            ],
            day,
        );
        let report = stats.topn_with_today(1, day);
        let b = report.domains.iter().find(|r| r.key == "b.com").unwrap();
        assert_eq!(b.up, 120);
    }

    #[test]
    fn apply_frame_splits_by_local_day() {
        let (stats, _dir) = open_temp();
        stats.apply_frame(&[item("c1", 100, 0, "a.com", "HK-01")], "2026-11-19");
        stats.apply_frame(&[item("c1", 300, 0, "a.com", "HK-01")], "2026-11-20");
        let prev = stats.topn_with_today(1, "2026-11-19");
        assert_eq!(prev.domains[0].up, 100);
        let today = stats.topn_with_today(1, "2026-11-20");
        assert_eq!(today.domains[0].up, 200); // 只计本日增量
        let week = stats.topn_with_today(7, "2026-11-20");
        assert_eq!(week.domains[0].up, 300); // 窗口合并全量
        assert_eq!(week.daily.len(), 2); // 逐日总量两桶
    }

    #[test]
    fn topn_ranks_and_caps() {
        let (stats, _dir) = open_temp();
        let day = "2026-11-20";
        let items: Vec<ConnItem> = (0..12)
            .map(|i| {
                item(
                    &format!("c{i}"),
                    1000 - i as u64 * 10,
                    0,
                    &format!("h{i}.com"),
                    "HK-01",
                )
            })
            .collect();
        stats.apply_frame(&items, day);
        let report = stats.topn_with_today(1, day);
        assert_eq!(report.domains.len(), TOPN);
        assert_eq!(report.domains[0].key, "h0.com");
        assert_eq!(report.nodes.len(), 1); // 单出口聚合成一条
        assert_eq!(report.nodes[0].key, "HK-01");
    }

    #[test]
    fn retention_trims_outside_window() {
        let (stats, dir) = open_temp();
        let today = "2026-11-20";
        let base = chrono::NaiveDate::from_ymd_opt(2026, 10, 11).unwrap();
        for i in 0..40u64 {
            let day = base
                .checked_add_days(chrono::Days::new(i))
                .unwrap()
                .format("%Y-%m-%d")
                .to_string();
            stats.apply_frame(&[item("c1", 10, 0, "a.com", "HK-01")], &day);
        }
        stats.apply_frame(&[item("c1", 10, 0, "a.com", "HK-01")], today);
        stats.flush_if_due(true);
        let file = fs::read_to_string(dir.path().join("traffic-stats.json")).unwrap();
        let saved: StatsFile = serde_json::from_str(&file).unwrap();
        assert!(saved.days.len() <= RETENTION_DAYS as usize);
        assert!(saved.days.contains_key(today));
        // 起始 40 天前已超出 30 天窗口
        assert!(!saved.days.contains_key("2026-10-11"));
    }

    #[test]
    fn flush_respects_throttle_but_force_writes() {
        let (stats, dir) = open_temp();
        let day = "2026-11-20";
        stats.apply_frame(&[item("c1", 10, 0, "a.com", "HK-01")], day);
        stats.flush_if_due(false); // 首次：last_flush 为 None，允许写
        assert!(dir.path().join("traffic-stats.json").is_file());
        stats.apply_frame(&[item("c1", 20, 0, "a.com", "HK-01")], day);
        stats.flush_if_due(false); // 刚写过：节流跳过
        let bytes = fs::read_to_string(dir.path().join("traffic-stats.json")).unwrap();
        assert!(bytes.contains(r#""a.com":[10,0]"#));
        stats.flush_if_due(true); // force 跳过时间闸
        let bytes = fs::read_to_string(dir.path().join("traffic-stats.json")).unwrap();
        assert!(bytes.contains(r#""a.com":[20,0]"#));
    }
}
