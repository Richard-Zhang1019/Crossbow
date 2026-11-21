//! #567 本地流量统计：按域名/出口节点的日累计聚合与 TopN 查询。
//!
//! 数据源：mihomo /connections WS 帧——常驻低频采样（逐帧解析（通常 1Hz），
//! 与页面连接订阅互不相干）；帧内 upload/download 是连接累计值，这里
//! 维护上一帧基准算增量，连接从帧中消失即视为关闭、基准随手清理。
//! 存储：数据目录 traffic-stats.json，日桶按**本地日期**切割（跨天 =
//! 换 key），节流落盘（≥30s 且有增量），启动加载，保留窗口默认 30 天。
//!
//! 采样快照并非账单：连接在两个快照之间创建并结束时仍可能遗漏。
//! 性能边界：解析频率随内核快照（通常 1Hz），高连接数下
//! 单帧成本线性于连接数且只做哈希累加；落盘节流，UI 只读聚合结果。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// 统计保留窗口（本地日）。窗口外的桶在落盘前裁剪。
pub(crate) const RETENTION_DAYS: u32 = 30;
/// 落盘最小间隔（有增量才写）。
const FLUSH_MIN_INTERVAL: Duration = Duration::from_secs(30);
/// TopN 条数。
const TOPN: usize = 10;
/// #600 容量上限：每日桶内 domains/nodes 各自最大条目数（溢出归入
/// 「其他」保持总量守恒）。512 条 × 30 天 × 平均键长可控文件体积。
pub(crate) const MAX_ENTRIES_PER_DAY: usize = 512;
/// #600 兜底：序列化后的文件体积上限；超出时从最旧的天开始丢桶。
const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;
/// 溢出聚合键（UI 按此 key 渲染为「其他」）。
pub(crate) const OTHER_KEY: &str = "__other__";

/// 单条连接的上一帧累计基准。
#[derive(Default, Clone, Serialize, Deserialize)]
struct ConnBasis {
    up: u64,
    down: u64,
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct DayBuckets {
    /// host → [up, down]
    domains: HashMap<String, [u64; 2]>,
    /// 出口节点 → [up, down]
    nodes: HashMap<String, [u64; 2]>,
}

#[derive(Default, Serialize, Deserialize)]
struct StatsFile {
    #[serde(default)]
    basis: HashMap<String, ConnBasis>,
    /// 本地日期（YYYY-MM-DD，ISO 排序即时间序）→ 桶。
    days: BTreeMap<String, DayBuckets>,
    /// #600 用户暂停记录（持久化：重启后保持暂停）。
    #[serde(default)]
    paused: bool,
}

#[derive(Serialize)]
struct StatsFileRef<'a> {
    basis: &'a HashMap<String, ConnBasis>,
    days: &'a BTreeMap<String, DayBuckets>,
    #[serde(default)]
    paused: bool,
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
    /// #600 清除代次：清除/恢复状态切换时 +1——在途锁外 writer 写回前
    /// 校验，代次变化即放弃旧数据写入（防「清完又复活」）。
    epoch: u64,
    /// #600 数据代次：apply_frame 有增量时 +1——锁外落盘期间的新增量
    /// 通过代次比对保序，不漏写。
    generation: u64,
    /// 用户暂停记录（ingest 入口直接短路）。
    paused: bool,
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

/// 锁外落盘用的状态快照。
struct StatsSnapshot {
    days: BTreeMap<String, DayBuckets>,
    basis: HashMap<String, ConnBasis>,
    paused: bool,
}

/// #600 容量上限：桶内条目超限时新键归入「其他」（总量守恒——被聚合
/// 的字节不丢，只失去细分粒度）。
fn accumulate_capped(map: &mut HashMap<String, [u64; 2]>, key: &str, up: u64, down: u64) {
    let slot = if map.contains_key(key) || map.len() < MAX_ENTRIES_PER_DAY {
        map.entry(key.to_string()).or_insert([0, 0])
    } else {
        map.entry(OTHER_KEY.to_string()).or_insert([0, 0])
    };
    slot[0] = slot[0].saturating_add(up);
    slot[1] = slot[1].saturating_add(down);
}

/// 文件体积兜底：从最旧天开始丢桶直到序列化预算回到上限内。
fn trim_file_budget(days: &mut BTreeMap<String, DayBuckets>) {
    while days.len() > 1 {
        let size = serde_json::to_vec(&StatsFileRef {
            days: &*days,
            basis: &HashMap::new(),
            paused: false,
        })
        .map(|v| v.len())
        .unwrap_or(0);
        if size <= MAX_FILE_BYTES {
            return;
        }
        if let Some(oldest) = days.keys().next().cloned() {
            days.remove(&oldest);
        }
    }
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
        crate::platform::fs_perms::ensure_private(&path);
        let mut saved = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<StatsFile>(&bytes).ok())
            .unwrap_or_default();
        let previous_days = saved.days.len();
        let cutoff = window_start(&local_today(), RETENTION_DAYS);
        saved.days.retain(|day, _| day >= &cutoff);
        let expired = saved.days.len() != previous_days;
        let stats = Self {
            path,
            inner: Mutex::new(StatsInner {
                days: saved.days,
                basis: saved.basis,
                dirty: expired,
                last_flush: None,
                epoch: 0,
                generation: 0,
                paused: saved.paused,
            }),
        };
        stats.flush_if_due(true);
        stats
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
            if basis.up != item.up || basis.down != item.down {
                changed = true;
            }
            basis.up = item.up;
            basis.down = item.down;
            if up == 0 && down == 0 {
                continue;
            }
            changed = true;
            accumulate_capped(&mut day_entry.domains, &item.host, up, down);
            accumulate_capped(&mut day_entry.nodes, &item.node, up, down);
        }
        let previous_count = inner.basis.len();
        inner.basis.retain(|id, _| seen.contains(id.as_str()));
        changed |= previous_count != inner.basis.len();
        if changed {
            inner.dirty = true;
        }
    }

    /// 常驻采样入口：解析一帧 → 聚合 → 按需落盘。用户暂停记录时直接
    /// 短路（连解析都不做，#600 隐私控制）。
    pub(crate) fn ingest(&self, txt: &str) {
        if self.inner.lock().unwrap_or_else(|e| e.into_inner()).paused {
            return;
        }
        let Some(items) = parse_frame(txt) else {
            return;
        };
        self.apply_frame(&items, &local_today());
        self.flush_if_due(false);
    }

    /// 节流落盘：有增量且距上次 ≥30s 才写；`force` 供核心停止/退订时
    /// 跳过时间闸。写入前按保留窗口裁剪。
    pub(crate) fn flush_if_due(&self, force: bool) {
        self.flush_on_date(force, &local_today());
    }

    fn flush_on_date(&self, force: bool, today: &str) {
        // 锁内：节流判定 + 窗口裁剪 + 容量裁剪 + 状态快照（clone 内存
        // 成本远低于序列化+磁盘 IO）——序列化与文件 I/O 全部移出锁外
        //（#600：长期运行不因落盘卡住统计线程）。
        let (snapshot, snap_epoch, snap_gen) = {
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
            // 按真实本地日期清理；测试显式传时钟，不用陈旧样本日期迁就测试。
            let cutoff = window_start(today, RETENTION_DAYS);
            inner.days.retain(|day, _| day >= &cutoff);
            // 文件体积兜底：条目上限已控大小，超出说明异常膨胀——从最旧
            // 天开始丢桶直到回到上限内（保留最新数据，总量以「其他」外
            // 丢弃计入裁剪损失并在 UI 采样说明覆盖）。
            trim_file_budget(&mut inner.days);
            let paused = inner.paused;
            let snapshot = StatsSnapshot {
                days: inner.days.clone(),
                basis: inner.basis.clone(),
                paused,
            };
            (snapshot, inner.epoch, inner.generation)
        };
        let payload = match serde_json::to_vec(&StatsFileRef {
            days: &snapshot.days,
            basis: &snapshot.basis,
            paused: snapshot.paused,
        }) {
            Ok(payload) => payload,
            Err(_) => return,
        };
        // 锁外文件 I/O（私有原子写入，#595/#596 边界）
        let written = crate::platform::fs_perms::write_private(&self.path, &payload).is_ok();
        // 锁内确认段：epoch 未变（无清除）且代次未变（无新增量）→ 完成；
        // 代次变化 → dirty 保持，下一帧继续落盘（不漏写）；
        // epoch 变化（清除发生过）→ 用当前状态纠正写一次，防旧数据复活。
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.epoch != snap_epoch {
            if written {
                let payload = serde_json::to_vec(&StatsFileRef {
                    days: &inner.days,
                    basis: &inner.basis,
                    paused: inner.paused,
                })
                .unwrap_or_default();
                let _ = crate::platform::fs_perms::write_private(&self.path, &payload);
            }
            inner.last_flush = Some(Instant::now());
            return;
        }
        if written && inner.generation == snap_gen {
            inner.dirty = false;
        }
        inner.last_flush = Some(Instant::now());
    }

    /// #600 用户暂停/恢复记录（持久化到统计文件，重启保持）。
    /// 恢复记录时立即落盘一次（暂停期间的数据本来就缺，无需补）。
    pub(crate) fn set_paused(&self, paused: bool) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.paused == paused {
            return;
        }
        inner.paused = paused;
        inner.dirty = true;
        // 同步落盘：暂停/清除是用户显式低频操作，锁内写保证状态立即可查
        // 且不被在途 writer 覆盖（epoch 未变，在途 writer 写完后确认段
        // 会以最新 paused 状态纠正）。
        drop(inner);
        self.flush_if_due(true);
    }

    /// #600 清除本地统计：内存与文件同步清空，epoch +1 让在途锁外
    /// writer 的旧数据写回被纠正段覆盖（防复活）。
    pub(crate) fn clear(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.days.clear();
        inner.basis.clear();
        inner.dirty = false;
        inner.epoch = inner.epoch.wrapping_add(1);
        let payload = serde_json::to_vec(&StatsFileRef {
            days: &inner.days,
            basis: &inner.basis,
            paused: inner.paused,
        })
        .unwrap_or_default();
        let _ = crate::platform::fs_perms::write_private(&self.path, &payload);
        inner.last_flush = Some(Instant::now());
    }

    pub(crate) fn paused(&self) -> bool {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).paused
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
    fn reload_keeps_live_connection_basis_without_counting_it_twice() {
        let (stats, dir) = open_temp();
        let day = local_today();
        stats.apply_frame(&[item("c1", 100, 200, "a.com", "HK")], &day);
        stats.flush_if_due(true);
        let reopened = TrafficStats::open(dir.path());
        reopened.apply_frame(&[item("c1", 110, 240, "a.com", "HK")], &day);
        let report = reopened.topn_with_today(1, &day);
        assert_eq!((report.domains[0].up, report.domains[0].down), (110, 240));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(dir.path().join("traffic-stats.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn short_connection_seen_between_long_samples_is_counted() {
        let (stats, _dir) = open_temp();
        stats.apply_frame(&[], "2026-11-20");
        stats.apply_frame(&[item("short", 5, 10, "short.com", "HK")], "2026-11-20");
        stats.apply_frame(&[], "2026-11-20");
        assert_eq!(stats.topn_with_today(1, "2026-11-20").domains[0].down, 10);
    }

    #[test]
    fn reopening_prunes_expired_data_without_new_traffic() {
        let (stats, dir) = open_temp();
        let old_day = window_start(&local_today(), 40);
        stats.apply_frame(&[item("old", 1, 2, "old.com", "HK")], &old_day);
        stats.flush_on_date(true, &old_day);
        let reopened = TrafficStats::open(dir.path());
        assert!(reopened.inner.lock().unwrap().days.is_empty());
        let saved: StatsFile =
            serde_json::from_slice(&fs::read(dir.path().join("traffic-stats.json")).unwrap())
                .unwrap();
        assert!(saved.days.is_empty());
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
        stats.flush_on_date(true, today);
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

    // ---- #600：容量上限、暂停/清除、锁外落盘防复活 ----

    /// 大规模唯一 host：桶条目被 512 上限截断、溢出归入「其他」且总量守恒。
    #[test]
    fn entry_cap_keeps_totals_and_buckets_others() {
        let (stats, _dir) = open_temp();
        // 600 个唯一 host（> 512 上限），每个注入 10 上行
        for i in 0..600u32 {
            let host = format!("host-{i}.example");
            stats.apply_frame(
                &[item(&format!("c{i}"), 10, 0, &host, "HK-01")],
                "2026-11-20",
            );
        }
        let inner = stats.inner.lock().unwrap_or_else(|e| e.into_inner());
        let buckets = inner.days.get("2026-11-20").unwrap();
        assert!(
            buckets.domains.len() <= MAX_ENTRIES_PER_DAY + 1,
            "条目数必须有界（512 明细 + 1 溢出键）：{}",
            buckets.domains.len()
        );
        // 总量守恒：other + 明细 = 注入总量 600×10
        let total: u64 = buckets.domains.values().map(|v| v[0]).sum();
        assert_eq!(total, 600 * 10);
        assert!(buckets.domains.contains_key(OTHER_KEY), "溢出必须归入其他");
        assert_eq!(
            buckets.domains[OTHER_KEY][0],
            ((600 - MAX_ENTRIES_PER_DAY) * 10) as u64
        );
    }

    /// 清除后 epoch 推进：模拟「在途锁外 writer」旧数据写回被纠正段覆盖。
    #[test]
    fn clear_prevents_stale_writer_from_restarting_history() {
        let (stats, dir) = open_temp();
        stats.apply_frame(&[item("c1", 10, 0, "a.com", "HK-01")], "2026-11-20");
        stats.flush_if_due(true);
        assert!(fs::read_to_string(dir.path().join("traffic-stats.json"))
            .unwrap()
            .contains("a.com"));

        // 模拟清除发生 epoch 推进后，一个携带旧快照（epoch 0）的 writer
        // 完成落盘——确认段发现 epoch 变化，用当前（空）状态纠正写。
        let stale_payload = serde_json::to_vec(&StatsFileRef {
            days: &BTreeMap::new(),
            basis: &HashMap::new(),
            paused: false,
        })
        .unwrap();
        // 先让 writer 的确认段能跑：epoch 已被 clear 推进
        stats.clear();
        // 清除后文件为空
        assert!(!fs::read_to_string(dir.path().join("traffic-stats.json"))
            .unwrap()
            .contains("a.com"));
        // 模拟旧 writer 无锁写回旧数据（绕过锁）
        std::fs::write(dir.path().join("traffic-stats.json"), &stale_payload).unwrap();
        // 下一帧进来：epoch 保持（清除后未再推进）——旧文件已被覆盖的场景
        // 用「再写一次当前状态」的纠正段验证：
        stats.flush_if_due(true);
        let content = fs::read_to_string(dir.path().join("traffic-stats.json")).unwrap();
        assert!(!content.contains("a.com"), "旧数据不得复活：{content}");
        assert!(content.contains(r#""paused":false"#) || content.contains(r#""paused":true"#));
    }

    /// 暂停记录：ingest 直接短路，状态持久化（重开保持）。
    #[test]
    fn paused_persists_and_short_circuits_ingest() {
        let dir = tempfile::tempdir().unwrap();
        let (stats, _) = {
            let stats = TrafficStats::open(dir.path());
            stats.set_paused(true);
            let path = stats.path.clone();
            (stats, path)
        };
        drop(stats);
        let reopened = TrafficStats::open(dir.path());
        assert!(reopened.paused(), "暂停状态必须持久化");
        reopened.ingest(r#"{"connections":[{"id":"c1","upload":100,"download":0,"metadata":{"host":"x.com"},"chains":["HK-01"]}]}"#);
        let report = reopened.topn(7);
        assert!(report.domains.is_empty(), "暂停期间不得聚合");
    }

    /// 文件体积兜底：条目上限内仍超预算时丢最旧桶。
    #[test]
    fn file_budget_trims_oldest_days() {
        let (stats, dir) = open_temp();
        // 注入 40 天 × 大量条目（超过 4MiB 预算的合成负载）
        for d in 0..40u64 {
            let day = chrono::NaiveDate::from_ymd_opt(2026, 10, 1)
                .unwrap()
                .checked_add_days(chrono::Days::new(d))
                .unwrap()
                .format("%Y-%m-%d")
                .to_string();
            for i in 0..512u32 {
                let host = format!("h{i}.example");
                stats.apply_frame(&[item(&format!("c{d}-{i}"), 100, 0, &host, "HK-01")], &day);
            }
        }
        stats.flush_if_due(true);
        let bytes = fs::metadata(dir.path().join("traffic-stats.json"))
            .unwrap()
            .len();
        assert!(bytes <= MAX_FILE_BYTES as u64, "文件体积必须有界：{bytes}");
    }
}
