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
    pub store: Mutex<Store>,
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
    /// 当前内核管理器快照（clone 便宜：内部是 Arc）。
    pub fn core(&self) -> CoreManager {
        self.core_slot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
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
}

// ---------- 内核与配置的公共操作（命令与托盘共用） ----------
