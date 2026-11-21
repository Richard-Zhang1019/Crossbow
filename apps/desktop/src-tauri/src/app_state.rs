// AppState 与流量采样（#423 Stage B 从 lib.rs 提取）。
use crate::engine::bridge::WsHub;
use crate::*;
use std::sync::Mutex;

pub struct AppState {
    pub(crate) instance_lock: Mutex<Option<std::fs::File>>,
    pub(crate) exit: crate::lifecycle::shutdown::ExitState,
    /// 轻量交接退出：RunEvent::Exit 时跳过清理（内核留给系统收养）。
    pub(crate) handoff: std::sync::atomic::AtomicBool,
    /// Explicit updater restart handoff; separate from the one-shot window guard.
    pub(crate) update_handoff: std::sync::atomic::AtomicBool,
    /// 安全模式：坏覆写导致内核启动失败后，跳过覆写链启动一次。
    pub(crate) safe_mode: std::sync::atomic::AtomicBool,
    pub(crate) config_transition: std::sync::atomic::AtomicBool,
    pub(crate) node_delay_revision: std::sync::atomic::AtomicU64,
    pub(crate) node_delay_task: Mutex<Option<NodeDelayTask>>,
    pub(crate) node_delay_slots: tokio::sync::Semaphore,
    pub store: Mutex<Store>,
    pub(crate) package_session: Mutex<crate::workflow::PackageSession>,
    /// 当前内核管理器；内核安装后整体替换（core_slot）。
    pub(crate) core_slot: Mutex<CoreManager>,
    /// 启动链操作锁：引擎换核(ensure)+渲染+启动必须整体串行，否则两个并发
    /// start_core（连接开关 vs 引擎切换）会交错出「A 换好的核被 B 换走，
    /// A 拿 B 的管理器启动 A 的配置」——即 mihomo 进程配 sing-box JSON。
    pub(crate) core_op: Mutex<()>,
    pub(crate) core_install_op: tauri::async_runtime::Mutex<()>,
    pub(crate) core_restart_op: tauri::async_runtime::Mutex<()>,
    /// 页面与托盘的选节点/恢复自动事务串行，避免解除固定与选择互相覆盖。
    pub(crate) proxy_op: Mutex<()>,
    pub(crate) sysproxy: SysProxyManager,
    pub(crate) activity_reporting: activity_reporting::Reporter,
    /// 内核运行模式：direct / rule / global。
    pub(crate) mode: Mutex<String>,
    #[cfg(not(target_os = "macos"))]
    pub(crate) tray: Mutex<Option<tauri::tray::TrayIcon<tauri::Wry>>>,
    pub(crate) ws: WsHub,
    pub(crate) data_dir: PathBuf,
    /// 托盘节点子菜单统一状态（此前 9 个分散锁的字段全部收拢于此）
    pub(crate) node_menu: Mutex<crate::tray::state::TrayState>,
    /// 应用句柄：换核/重装后给新 CoreManager 重挂状态回调用。
    pub(crate) app: AppHandle,
    /// 更新设置（#28：自动检查/自动下载）。
    pub(crate) update: Mutex<crossbow_core::model::UpdateSettings>,
    /// 速率历史环形缓冲（#181）：托盘通道每秒喂入，与页面存亡无关；
    /// 首页挂载时一次取回预填「最近 120 秒」图表。
    pub(crate) traffic_history: Mutex<std::collections::VecDeque<TrafficSample>>,
    /// 本地流量统计（#567）：按域名/出口节点的日累计聚合，常驻低频采样。
    pub(crate) traffic_stats: traffic_stats::TrafficStats,
}

/// 单秒速率采样（字节/秒）。
#[derive(serde::Serialize, Clone, Copy)]
pub struct TrafficSample {
    pub up: u64,
    pub down: u64,
}

/// 速率历史环形缓冲容量 = 前端图表窗口（最近 120 秒）。
const TRAFFIC_HISTORY_LEN: usize = 120;

impl AppState {
    pub(crate) fn begin_config_transition(
        &self,
    ) -> crate::engine::config_transition::RecoveryOwner<'_> {
        let owner = crate::engine::config_transition::RecoveryOwner::enter(&self.config_transition);
        self.node_delay_revision
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        owner
    }

    /// 当前内核管理器快照（clone 便宜：内部是 Arc）。
    pub fn core(&self) -> CoreManager {
        self.core_slot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Shared by the desktop node page and the tray before and after a delay request.
    pub(crate) fn node_delay_session_current(&self, pid: Option<u32>) -> bool {
        let core = self.core();
        self.exit.ensure_open().is_ok()
            && !self
                .config_transition
                .load(std::sync::atomic::Ordering::SeqCst)
            && pid.is_some()
            && core.pid() == pid
            && core.status() == CoreStatus::Running
    }

    /// 追加一秒采样，超出窗口即淘汰最旧（#181）
    pub fn traffic_history_push(&self, up: u64, down: u64) {
        let mut ring = self
            .traffic_history
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        ring.push_back(TrafficSample { up, down });
        while ring.len() > TRAFFIC_HISTORY_LEN {
            ring.pop_front();
        }
    }

    /// 快照（旧 → 新）。内核停止时清空，图表诚实回到空态。
    pub fn traffic_history_snapshot(&self) -> Vec<TrafficSample> {
        self.traffic_history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .copied()
            .collect()
    }

    pub fn traffic_history_clear(&self) {
        self.traffic_history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// #567：解析一帧 /connections 并聚合（常驻采样线程逐帧喂入）。
    pub fn traffic_stats_ingest(&self, txt: &str) {
        self.traffic_stats.ingest(txt);
    }

    /// #567：核心停止/退订——立即强制落盘，保留窗口裁剪。
    pub fn traffic_stats_stop(&self) {
        self.traffic_stats.flush_if_due(true);
    }

    /// #567：窗口内 TopN + 逐日总量（前端统计页查询）。
    pub fn traffic_stats_topn(&self, days: u32) -> traffic_stats::TrafficStatsReport {
        self.traffic_stats.topn(days)
    }

    /// #600：暂停/恢复本地流量记录（持久化到统计文件）。
    pub fn traffic_stats_set_paused(&self, paused: bool) {
        self.traffic_stats.set_paused(paused);
    }

    /// #600：清除本地统计（内存与文件同步清空，epoch 防复活）。
    pub fn traffic_stats_clear(&self) {
        self.traffic_stats.clear();
    }

    pub fn traffic_stats_paused(&self) -> bool {
        self.traffic_stats.paused()
    }
}

// ---------- 内核与配置的公共操作（命令与托盘共用） ----------
