//! 内核 sidecar 生命周期管理：启动、就绪探测、停止、崩溃自动重启。
//!
//! 刻意不依赖 Tauri，便于直接单元测试与集成测试（真实内核冒烟）。
//! 平台注意：Windows 下 spawn 需带 CREATE_NO_WINDOW，避免 GUI 进程闪控制台。

use std::collections::VecDeque;
use std::io::Write;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Child;
#[cfg(test)]
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbow_core::{apply_runtime, RuntimeConfig};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "state", content = "message")]
pub enum CoreStatus {
    Stopped,
    Starting,
    Running,
    Crashed(String),
}

/// 可调参数；测试用短超时。
#[derive(Debug, Clone)]
pub struct CoreOptions {
    /// 就绪探测超时（秒）；超时视为启动失败（不自动重启，直接上报）。
    /// 120s：首次启动 mihomo 要自行下载 GeoIP 数据库（数 MB），
    /// 我们的应用侧镜像下载失败时它是最慢的兜底路径。
    pub readiness_timeout_secs: u64,
    /// 崩溃重启的统计窗口（秒）。
    pub restart_window_secs: u64,
    /// 窗口内最大重启次数，超过后放弃并保持 Crashed。
    pub max_restarts: u32,
    /// 启动前确保 GeoIP/GeoSite 数据文件（真实环境 true；测试 false）。
    pub ensure_geo_files: bool,
}

impl Default for CoreOptions {
    fn default() -> Self {
        Self {
            readiness_timeout_secs: 120,
            restart_window_secs: 60,
            max_restarts: 3,
            ensure_geo_files: true,
        }
    }
}

pub type Callback = Arc<dyn Fn(&CoreStatus) + Send + Sync>;

struct Inner {
    child: Option<Child>,
    status: CoreStatus,
    /// 手动停止标记：watch 线程看到退出时不重启。
    stopping: bool,
    restarts: VecDeque<Instant>,
    #[cfg(target_os = "macos")]
    tun_health_restart: Option<(u32, Instant)>,
    /// (port, secret)
    controller: Option<(u16, String)>,
    /// 轻量交接中：内核不再归本进程所有（退出时不杀）。
    detached: bool,
    /// 收养的外部内核 pid（上一个 GUI 实例留下的）。
    external_pid: Option<u32>,
    /// 收养进程的可执行文件身份；对外部 PID 发信号/提权前必须校验。
    external_binary: Option<PathBuf>,
}

/// 线程间共享状态；CoreManager 可 Clone。
struct Shared {
    binary_path: PathBuf,
    /// 内核工作目录（mihomo `-d`），config.yaml 写在这里。
    work_dir: PathBuf,
    opts: Mutex<CoreOptions>,
    callback: Mutex<Option<Callback>>,
    inner: Mutex<Inner>,
    /// 启停与服务探测收尾共用；不能让服务停的迟到回包清掉下一批次的租约。
    /// 锁序为 lifecycle -> inner，网络就绪探测期间不持有 lifecycle。
    lifecycle: Mutex<()>,
    /// 本次启动链的最终配置内容；spawn（含崩溃重启）每次都据此重写
    /// config.yaml，避免换引擎后旧管理器复用新引擎的配置文件。
    config_cache: Mutex<Option<String>>,
    /// 测试注入服务停止。生产构建不带这个钩子，避免测试替身进入发布二进制。
    #[cfg(test)]
    service_stop_hook: Mutex<Option<ServiceStopHook>>,
}

/// 服务托管启动的进程拉起器（Windows/Linux TUN 路径注入）。
pub(crate) type CoreSpawner = Box<dyn FnOnce(&Path, &Path) -> Result<u32, String> + Send>;
/// 测试注入的服务停止钩子。
#[cfg(test)]
type ServiceStopHook = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

impl Shared {
    fn external_start_is_current(&self, pid: u32, probe: &ControllerProbe) -> bool {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        !inner.stopping
            && inner.external_pid == Some(pid)
            && inner
                .controller
                .as_ref()
                .is_some_and(|(port, secret)| *port == probe.port && secret == &probe.secret)
    }

    /// 生产路径请 Windows 服务停内核。测试可注入替身，避免连到本机真实管道。
    fn request_service_stop(&self) -> Result<(), String> {
        #[cfg(test)]
        {
            let hook = self
                .service_stop_hook
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            if let Some(hook) = hook {
                return hook();
            }
        }
        #[cfg(windows)]
        {
            return crate::service_client::stop_core();
        }
        #[cfg(not(windows))]
        {
            Err("service stop is only available on Windows".into())
        }
    }

    /// 重启或就绪超时前收走服务托管租约。调用方持有 lifecycle，
    /// 保证服务停止和清掉 PID/描述文件期间不会开始下一批次。
    fn release_hosted_before_kill(&self) -> Result<crate::engine::restart::Cleanup, String> {
        let (has_child, pid, binary) = {
            let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            (
                inner.child.is_some(),
                inner.external_pid,
                inner.external_binary.clone(),
            )
        };
        let cleanup = crate::engine::restart::cleanup_before_restart(
            has_child,
            pid,
            binary.as_deref(),
            || self.request_service_stop(),
            || pid.is_some_and(pid_is_alive),
        )?;
        if cleanup == crate::engine::restart::Cleanup::Released {
            {
                let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                inner.external_pid = None;
                inner.external_binary = None;
                inner.detached = false;
            }
            let _ = std::fs::remove_file(Self::descriptor_path(&self.work_dir));
        }
        Ok(cleanup)
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        // 兜底防孤儿：最后一个管理器句柄丢弃（含 panic 路径）时收走内核进程。
        // 轻量交接中例外：内核已有意留给系统收养。
        {
            let mut inner = self.inner.lock().unwrap();
            inner.stopping = true;
            if inner.detached {
                return;
            }
        }
        self.kill_child();
    }
}

impl Shared {
    fn descriptor_path(work_dir: &Path) -> PathBuf {
        work_dir.join("core-descriptor.json")
    }

    /// 写轻量交接描述符（port/secret/pid），下次 GUI 启动据此收养。
    /// 不获取 inner 锁，允许调用方在状态锁内完成原子交接。
    fn write_descriptor(
        &self,
        pid: u32,
        port: u16,
        secret: &str,
        binary: &Path,
    ) -> Result<(), String> {
        let record = serde_json::json!({
            "port": port, "secret": secret, "pid": pid,
            "binary_path": binary.display().to_string(),
            "config": self.config_cache.lock().unwrap().clone(),
        });
        // A restart must not see a truncated descriptor. Keep credentials private.
        let temporary = self
            .work_dir
            .join(format!(".core-descriptor-{}.tmp", Uuid::new_v4()));
        let result = (|| {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary).map_err(|e| e.to_string())?;
            file.write_all(&serde_json::to_vec(&record).map_err(|e| e.to_string())?)
                .and_then(|()| file.sync_all())
                .map_err(|e| e.to_string())?;
            drop(file);
            std::fs::rename(&temporary, Self::descriptor_path(&self.work_dir))
                .map_err(|e| e.to_string())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(temporary);
        }
        result
    }

    fn notify(&self, status: CoreStatus) {
        self.inner.lock().unwrap().status = status.clone();
        // 回调可能等待 core_op，不能持注册锁调用。
        let callback = self.callback.lock().unwrap().clone();
        if let Some(cb) = callback {
            cb(&status);
        }
    }

    fn spawn_child(&self, previous: Option<u32>) -> Result<Option<(u32, u16, String)>, String> {
        let mut inner = self.inner.lock().unwrap();
        if inner.stopping || inner.detached || inner.child.as_ref().map(Child::id) != previous {
            return Ok(None);
        }
        let (port, secret) = inner.controller.clone().ok_or("controller state missing")?;
        let config_path = self.work_dir.join("config.yaml");
        // 每次拉起（含崩溃自动重启）都重写配置：内容来自本次启动链的缓存，
        // 保证磁盘文件与当前管理的内核/引擎严格一致。
        {
            let cached = self.config_cache.lock().unwrap();
            let Some(content) = cached.as_ref() else {
                return Err("no config cached for this start chain".into());
            };
            std::fs::File::create(&config_path)
                .and_then(|mut f| f.write_all(content.as_bytes()))
                .map_err(|e| format!("write config: {e}"))?;
        }
        let mut cmd = crate::process::command(&self.binary_path);
        // 两内核 CLI 不同：mihomo `-d workdir -f config`；sing-box `run -D workdir -c config`
        if crate::engine::download::is_core_bin(&self.binary_path, "sing-box") {
            cmd.arg("run")
                .arg("-D")
                .arg(&self.work_dir)
                .arg("-c")
                .arg(&config_path);
        } else {
            cmd.arg("-d")
                .arg(&self.work_dir)
                .arg("-f")
                .arg(&config_path);
        }
        // 内核输出落到文件：崩溃诊断（yaml 错误等）靠它，不再丢弃。
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(self.work_dir.join("core-stdio.log"))
            .map_err(|e| format!("open core log: {e}"))?;
        let log_err = log_file
            .try_clone()
            .map_err(|e| format!("clone core log: {e}"))?;
        cmd.stdout(log_file).stderr(log_err);
        let child = cmd.spawn().map_err(|e| format!("spawn core: {e}"))?;
        let pid = child.id();
        inner.external_pid = None;
        inner.detached = false;
        inner.child = Some(child);
        if let Err(error) = self.write_descriptor(pid, port, &secret, &self.binary_path) {
            eprintln!("could not persist core descriptor: {error}");
        }
        drop(inner);
        append_decision(
            &self.work_dir,
            &format!(
                "spawned: pid={pid} bin={} cfg_head={:?}",
                self.binary_path.display(),
                std::fs::read_to_string(&config_path)
                    .map(|c| c.chars().take(40).collect::<String>())
                    .unwrap_or_default(),
            ),
        );
        Ok(Some((pid, port, secret)))
    }

    fn kill_child(&self) -> bool {
        let (child, external) = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let child = inner.child.take();
            let external = inner
                .external_pid
                .take()
                .map(|pid| (pid, inner.external_binary.take()));
            (child, external)
        };

        let stopped = if let Some(mut child) = child {
            let pid = child.id();
            #[cfg(unix)]
            let mut exited = || child.try_wait().is_ok_and(|status| status.is_some());
            #[cfg(unix)]
            let outcome = terminate_unix(pid, &mut exited, || true, TERM_GRACE);
            #[cfg(target_os = "macos")]
            let mut outcome = outcome;
            #[cfg(not(unix))]
            let gone = {
                let _ = child.kill();
                let mut exited = || child.try_wait().is_ok_and(|status| status.is_some());
                wait_for_exit(&mut exited, KILL_GRACE)
            };
            #[cfg(target_os = "macos")]
            if outcome == StopOutcome::PermissionDenied {
                if let Some(admin) = process_start(pid)
                    .and_then(|started| escalate_admin_stop(pid, &self.binary_path, &started))
                {
                    outcome = if wait_for_exit(&mut exited, KILL_GRACE) {
                        admin
                    } else {
                        StopOutcome::Unconfirmed
                    };
                }
            }
            #[cfg(unix)]
            let gone = outcome.confirmed();
            #[cfg(unix)]
            append_decision(
                &self.work_dir,
                &format!("core stop: pid={pid} outcome={outcome:?}"),
            );
            if gone {
                let _ = child.wait();
                true
            } else {
                // Preserve enough identity to retry safely; the child handle itself
                // cannot be retained after this bounded stop attempt.
                let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                inner.external_pid = Some(pid);
                inner.external_binary = Some(self.binary_path.clone());
                inner.detached = false;
                false
            }
            // gone=false 时故意丢弃 Child：进程仍存活，句柄无再收价值
        } else if let Some((pid, binary)) = external {
            #[cfg(target_os = "linux")]
            let stopped = if crate::network::linux_service::managed_pid() == Some(pid) {
                match crate::network::linux_service::stop_core() {
                    Ok(stopped) => stopped,
                    Err(error) => {
                        eprintln!("Linux service cleanup failed: {error}");
                        false
                    }
                }
            } else {
                kill_external_checked(pid, binary.as_deref(), Some(&self.work_dir))
            };
            #[cfg(not(target_os = "linux"))]
            let stopped = kill_external_checked(pid, binary.as_deref(), Some(&self.work_dir));
            if !stopped {
                let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                inner.external_pid = Some(pid);
                inner.external_binary = binary;
                inner.detached = false;
            }
            stopped
        } else {
            true
        };

        if stopped {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.detached = false;
            let _ = std::fs::remove_file(Self::descriptor_path(&self.work_dir));
        }
        stopped
    }
}

#[derive(Clone)]
pub struct CoreManager {
    shared: Arc<Shared>,
}
impl CoreManager {
    pub fn new(binary_path: PathBuf, work_dir: PathBuf) -> Self {
        Self {
            shared: Arc::new(Shared {
                binary_path,
                work_dir,
                opts: Mutex::new(CoreOptions::default()),
                callback: Mutex::new(None),
                lifecycle: Mutex::new(()),
                config_cache: Mutex::new(None),
                #[cfg(test)]
                service_stop_hook: Mutex::new(None),
                inner: Mutex::new(Inner {
                    child: None,
                    status: CoreStatus::Stopped,
                    stopping: false,
                    restarts: VecDeque::new(),
                    #[cfg(target_os = "macos")]
                    tun_health_restart: None,
                    controller: None,
                    detached: false,
                    external_pid: None,
                    external_binary: None,
                }),
            }),
        }
    }

    /// 测试与设置页注入参数；当前生产路径用默认值。
    #[allow(dead_code)]
    pub fn set_options(&self, opts: CoreOptions) {
        *self.shared.opts.lock().unwrap() = opts;
    }

    #[cfg(test)]
    fn set_service_stop_hook(&self, hook: impl Fn() -> Result<(), String> + Send + Sync + 'static) {
        *self.shared.service_stop_hook.lock().unwrap() = Some(Arc::new(hook));
    }

    pub fn set_callback(&self, cb: Callback) {
        *self.shared.callback.lock().unwrap() = Some(cb);
    }

    pub fn status(&self) -> CoreStatus {
        self.shared.inner.lock().unwrap().status.clone()
    }

    /// 当前控制器 (port, secret)；S3 的 traffic/logs WS 订阅会用到。
    #[allow(dead_code)]
    pub fn controller(&self) -> Option<(u16, String)> {
        self.shared.inner.lock().unwrap().controller.clone()
    }

    /// 内核二进制路径（诊断与版本查询用）。
    pub fn binary_path(&self) -> PathBuf {
        self.shared
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .external_binary
            .clone()
            .unwrap_or_else(|| self.shared.binary_path.clone())
    }

    /// 本次启动链的最终配置内容（当前出口解析等用途）；未启动过返回 None。
    pub fn current_config(&self) -> Option<String> {
        self.shared.config_cache.lock().unwrap().clone()
    }

    /// 本次启动使用的配置文件。热重载必须写回这个路径，内核才认。
    pub fn config_path(&self) -> PathBuf {
        self.shared.work_dir.join("config.yaml")
    }

    /// 覆盖配置文件，不更新崩溃重启用的缓存。
    /// 热重载成功后才调用 `replace_cached_config`，失败时缓存仍是旧内容。
    pub fn write_config_file(&self, content: &str) -> Result<(), String> {
        std::fs::write(self.config_path(), content).map_err(|e| format!("write config: {e}"))
    }

    pub fn replace_cached_config(&self, content: String) {
        *self
            .shared
            .config_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(content);
        let inner = self.shared.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let (Some(pid), Some((port, secret))) = (
            inner
                .child
                .as_ref()
                .map(|child| child.id())
                .or(inner.external_pid),
            inner.controller.as_ref(),
        ) {
            let binary = inner
                .external_binary
                .as_deref()
                .unwrap_or(&self.shared.binary_path);
            if let Err(error) = self.shared.write_descriptor(pid, *port, secret, binary) {
                eprintln!("could not persist updated core descriptor: {error}");
            }
        }
    }

    /// 启动链决策日志（独立文件 core-decision.log），排查引擎切换竞态。
    pub fn log_decision(&self, line: &str) {
        append_decision(&self.shared.work_dir, line);
    }

    /// 当前内核 PID，包括收养或服务托管的进程。
    #[allow(dead_code)]
    pub fn pid(&self) -> Option<u32> {
        let inner = self.shared.inner.lock().unwrap();
        inner.child.as_ref().map(|c| c.id()).or(inner.external_pid)
    }

    /// 启动内核。`base_config` 是渲染后的订阅配置（未经运行时注入）。
    pub fn start(&self, base_config: &str, engine_rt: &RuntimeConfig) -> Result<(), String> {
        self.start_internal(base_config, engine_rt, None)
    }

    /// 服务托管变体（Windows/Linux TUN）：「拉起进程」外包给特权服务，
    /// 端口/密钥生成、配置注入、就绪探测与状态簿记与常规路径完全一致。
    // macOS 非 test 构建不用服务托管路径；Windows/Linux 运行时与 mac 测试都要
    #[cfg(any(windows, target_os = "linux", test))]
    pub fn start_with_spawner(
        &self,
        base_config: &str,
        engine_rt: &RuntimeConfig,
        spawner: CoreSpawner,
    ) -> Result<(), String> {
        self.start_internal(base_config, engine_rt, Some(spawner))
    }

    fn start_internal(
        &self,
        base_config: &str,
        engine_rt: &RuntimeConfig,
        spawner: Option<CoreSpawner>,
    ) -> Result<(), String> {
        let _lifecycle = self
            .shared
            .lifecycle
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let result = self.start_attempt(base_config, engine_rt, spawner);
        if let Err(error) = &result {
            if self.status() == CoreStatus::Starting {
                self.shared.notify(CoreStatus::Crashed(error.clone()));
            }
        }
        result
    }

    fn start_attempt(
        &self,
        base_config: &str,
        engine_rt: &RuntimeConfig,
        spawner: Option<CoreSpawner>,
    ) -> Result<(), String> {
        // 上一次尝试还在探测中（Starting）：先收掉再启动，避免新配置写入后
        // 被旧探测超时路径误杀/误报。
        if self.status() == CoreStatus::Starting && !self.stop_locked() {
            return Err("无法确认上一次启动的内核已退出，已中止新启动".into());
        }
        if !matches!(self.status(), CoreStatus::Stopped | CoreStatus::Crashed(_)) {
            return Err(format!("core is {:?}, stop it first", self.status()));
        }
        if !self.shared.binary_path.exists() {
            let msg = format!(
                "core binary not found: {}",
                self.shared.binary_path.display()
            );
            self.shared.notify(CoreStatus::Crashed(msg.clone()));
            return Err(msg);
        }
        // 服务托管租约必须先请服务停。kill_child 对没有身份的活 PID 会直接拒绝，
        // 如果写在服务停止前面，SYSTEM 内核会把后续启动永久卡住（#343）。
        if let Err(msg) = self.shared.release_hosted_before_kill() {
            self.shared.notify(CoreStatus::Crashed(msg.clone()));
            return Err(msg);
        }
        let has_tracked_process = {
            let inner = self.shared.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.child.is_some() || inner.external_pid.is_some()
        };
        if has_tracked_process && !self.shared.kill_child() {
            let msg = crate::engine::restart::RESTART_BLOCKED.to_string();
            self.shared.notify(CoreStatus::Crashed(msg.clone()));
            return Err(msg);
        }
        // 外部（上一实例轻量交接的）内核还活着：先收掉，避免端口冲突。
        // 先解除收养角色再杀，收养监视线程看到角色已换就静默退场，不会误报；
        // 杀失败保留描述文件并中止启动，避免无登记孤儿内核抢端口。
        if let Some(descriptor) = Shared::read_descriptor(&self.shared.work_dir) {
            {
                let mut inner = self.shared.inner.lock().unwrap_or_else(|e| e.into_inner());
                if inner.external_pid == Some(descriptor.pid) {
                    inner.external_pid = None;
                    inner.external_binary = None;
                    inner.detached = false;
                }
            }
            // Windows SYSTEM 内核 GUI 杀不动：先请服务停（服务没托管它时
            // 回 Err，照常走常规 kill_external——普通内核路径不变）
            #[cfg(target_os = "windows")]
            let stopped_via_service = self.shared.request_service_stop().is_ok();
            #[cfg(not(target_os = "windows"))]
            let stopped_via_service = false;
            if !stopped_via_service
                && !kill_external_checked(
                    descriptor.pid,
                    descriptor.binary_path.as_deref(),
                    Some(&self.shared.work_dir),
                )
            {
                let msg = format!(
                    "无法确认旧内核进程 {} 已退出；描述文件已保留，请关闭旧内核后重试",
                    descriptor.pid
                );
                self.shared.notify(CoreStatus::Crashed(msg.clone()));
                return Err(msg);
            }
            let _ = std::fs::remove_file(Shared::descriptor_path(&self.shared.work_dir));
        }
        std::fs::create_dir_all(&self.shared.work_dir).map_err(|e| e.to_string())?;

        // Port handover is part of startup too. A privileged macOS listener can
        // leave cross-UID TIME_WAIT sockets for ~60s after confirmed process exit.
        self.shared.notify(CoreStatus::Starting);

        // 混合端口预检：被其他程序占用（常见：Clash Verge 等代理同端口）时，
        // sing-box 会 FATAL 退出、mihomo 静默失去代理能力——直接给出可操作的错误。
        // 常见占用者其实是本应用自己的残留内核（TUN root 内核 EPERM 杀不掉）：
        // 先按可执行路径识别并清理（root 走管理员补杀），重试一次，仍失败才报错。
        let address = if engine_rt.allow_lan {
            SocketAddr::from(([0, 0, 0, 0], engine_rt.mixed_port))
        } else {
            SocketAddr::from(([127, 0, 0, 1], engine_rt.mixed_port))
        };
        let try_bind = || check_mixed_port_available(address);
        if wait_for_mixed_port(&try_bind, Duration::from_secs(5)).is_err() {
            // 候选：当前内核 + 应用包内内核 + 数据目录内核 + tun-binaries 各版本副本
            // （实测踩坑：测试用 bundle 内核占端口时，仅 tun-binaries 候选会漏匹配）
            let mut candidates: Vec<PathBuf> = vec![self.shared.binary_path.clone()];
            if let Ok(exe) = std::env::current_exe() {
                if let Some(dir) = exe.parent() {
                    for name in ["mihomo", "sing-box"] {
                        candidates.push(dir.join(crate::engine::download::core_bin_file(name)));
                    }
                }
            }
            if let Some(data_dir) = self.shared.work_dir.parent() {
                let tb = data_dir.join("tun-binaries");
                if let Ok(versions) = std::fs::read_dir(&tb) {
                    for v in versions.flatten() {
                        for name in ["mihomo", "sing-box"] {
                            candidates
                                .push(v.path().join(crate::engine::download::core_bin_file(name)));
                        }
                    }
                }
                for name in ["mihomo", "sing-box"] {
                    candidates.push(data_dir.join(crate::engine::download::core_bin_file(name)));
                }
            }
            candidates.sort();
            candidates.dedup();
            let mut note = String::new();
            let mut stop_failed = false;
            for (pid, path) in find_leftover_core_pids(&candidates) {
                if kill_external_checked(pid, Some(&path), Some(&self.shared.work_dir)) {
                    note.push_str(&format!(
                        "已清理本应用残留内核 pid={pid}（{}）；",
                        path.display()
                    ));
                } else {
                    stop_failed = true;
                    note.push_str(&format!(
                        "残留内核 pid={pid}（{}）清理失败（可能取消了管理员授权）；",
                        path.display()
                    ));
                }
            }
            // Root-owned Crossbow kernels may be invisible to process enumeration.
            // The privileged fallback only targets this app's own binary paths.
            if wait_for_mixed_port(&try_bind, Duration::from_secs(5)).is_err()
                && port_has_listener(address)
                && !stop_failed
                && escalate_clear_port(engine_rt.mixed_port, &candidates, &self.shared.work_dir)
            {
                note.push_str("已通过管理员权限清理端口上的残留内核；");
            }
            // Windows：卸载流程不杀内核子进程，旧安装的内核（可能装在别的目录）
            // 常驻占端口——路径匹配会漏。netstat 定位端口监听者，镜像名精确
            // 匹配本应用内核（mihomo.exe/sing-box.exe）后 taskkill 清理。
            #[cfg(target_os = "windows")]
            if wait_for_mixed_port(&try_bind, Duration::from_secs(5)).is_err() {
                for (pid, image) in windows_port_listeners(engine_rt.mixed_port) {
                    let image_l = image.to_lowercase();
                    if image_l == "mihomo.exe" || image_l == "sing-box.exe" {
                        let killed = std::process::Command::new("taskkill")
                            .args(["/F", "/PID", &pid.to_string()])
                            .output()
                            .map(|o| o.status.success())
                            .unwrap_or(false);
                        if killed {
                            note.push_str(&format!("已清理本应用残留内核 {image}（pid={pid}）；"));
                        } else {
                            note.push_str(&format!(
                                "残留内核 {image}（pid={pid}）清理失败（可能需要管理员权限）；"
                            ));
                        }
                    } else {
                        note.push_str(&format!("占用者 {image}（pid={pid}）；"));
                    }
                }
            }
            // After a TUN core exits, accepted macOS sockets can keep this
            // address unavailable briefly even though no process is listening.
            // Wait for that handover instead of reporting a false conflict.
            let handover_wait = if port_has_listener(address) {
                Duration::from_secs(5)
            } else {
                Duration::from_secs(if cfg!(target_os = "macos") { 75 } else { 30 })
            };
            if let Err(e) = wait_for_mixed_port(&try_bind, handover_wait) {
                let listeners = port_listeners_via_lsof(engine_rt.mixed_port);
                for (pid, cmd) in &listeners {
                    note.push_str(&format!("占用者 pid={pid}（{cmd}）；"));
                }
                let message = format!(
                    "混合端口 {} 无法绑定（{e}）。{note}若仍失败：请更换本应用的混合端口，或退出占用该端口的程序",
                    engine_rt.mixed_port
                );
                self.shared.notify(CoreStatus::Crashed(message.clone()));
                return Err(message);
            }
        }
        // controller 端口避开 mixed，避免自相争抢
        let mut port = free_port().ok_or("no free port for controller")?;
        while port == engine_rt.mixed_port {
            port = free_port().ok_or("no free port for controller")?;
        }
        let secret = Uuid::new_v4().simple().to_string();
        let rt = RuntimeConfig {
            controller_port: port,
            controller_secret: secret.clone(),
            ..engine_rt.clone()
        };
        let final_config = apply_runtime(base_config, &rt)?;
        // 配置随启动链缓存：崩溃自动重启也重写文件，杜绝旧内核配新配置
        *self.shared.config_cache.lock().unwrap() = Some(final_config.clone());
        append_decision(
            &self.shared.work_dir,
            &format!(
                "start: bin={} engine={:?} mixed={} ctrl={} cfg_head={:?}",
                self.shared.binary_path.display(),
                rt.engine,
                rt.mixed_port,
                port,
                final_config.chars().take(48).collect::<String>(),
            ),
        );

        {
            let mut inner = self.shared.inner.lock().unwrap();
            inner.stopping = false;
            inner.controller = Some((port, secret.clone()));
            inner.restarts.clear();
        }
        match spawner {
            None => spawn_and_watch(self.shared.clone(), None),
            Some(spawner) => {
                // 与 spawn_child 相同约定：配置写盘 + 日志路径，交给服务拉起
                let config_path = self.shared.work_dir.join("config.yaml");
                std::fs::File::create(&config_path)
                    .and_then(|mut f| f.write_all(final_config.as_bytes()))
                    .map_err(|e| format!("write config: {e}"))?;
                let log_path = self.shared.work_dir.join("core-stdio.log");
                self.shared.notify(CoreStatus::Starting);
                let pid = match spawner(&config_path, &log_path) {
                    Ok(pid) => pid,
                    Err(error) => {
                        self.shared
                            .notify(CoreStatus::Crashed(format!("服务拉起内核失败：{error}")));
                        return Err(error);
                    }
                };
                // 描述文件必须记服务实际拉起的二进制。记成界面选中的另一份内核时，
                // 下次收养会把仍在运行的 SYSTEM 进程当成已经退出。
                #[cfg(windows)]
                let actual_binary = crate::service_client::pinned_core_path()
                    .unwrap_or_else(|_| self.shared.binary_path.clone());
                #[cfg(target_os = "linux")]
                let actual_binary = crate::network::linux_service::binary_path();
                #[cfg(not(any(windows, target_os = "linux")))]
                let actual_binary = self.shared.binary_path.clone();
                {
                    let mut inner = self.shared.inner.lock().unwrap();
                    inner.external_pid = Some(pid);
                    inner.detached = true;
                    inner.external_binary = Some(actual_binary.clone());
                }
                if let Err(error) = self
                    .shared
                    .write_descriptor(pid, port, &secret, &actual_binary)
                {
                    eprintln!("could not persist core descriptor: {error}");
                }
                // SYSTEM 内核归服务管理，这里只探测置 Running（无子进程句柄
                // 可监视；死亡可感知性由 #20 健康监测的控制器探测兜底）
                let shared = self.shared.clone();
                crate::lifecycle::authorization::spawn_background(move || {
                    probe_external_core(shared, pid, port, secret);
                });
                #[cfg(target_os = "linux")]
                watch_linux_service(self.shared.clone(), pid);
            }
        }
        Ok(())
    }

    /// 启动时收养上一实例轻量交接的内核：探测通过则标记为运行中的外部实例。
    pub fn adopt_external(&self) -> bool {
        let Some(d) = Shared::read_descriptor(&self.shared.work_dir) else {
            return false;
        };
        // A Linux TUN lease ends with its owning GUI connection. Never adopt
        // that capability-bearing process as an ordinary detached core.
        #[cfg(target_os = "linux")]
        if d.binary_path.as_ref() == Some(&crate::network::linux_service::binary_path()) {
            return false;
        }
        // Do not block GUI startup probing a descriptor whose process already exited
        // or whose PID was reused by a different binary.
        let gone = || {
            !pid_is_alive(d.pid)
                || d.binary_path
                    .as_ref()
                    .is_some_and(|path| process_matches_binary(d.pid, path) == Some(false))
        };
        if gone() {
            let _ = std::fs::remove_file(Shared::descriptor_path(&self.shared.work_dir));
            return false;
        }
        let probe = ControllerProbe {
            port: d.port,
            secret: d.secret.clone(),
        };
        let timeout = if cfg!(windows) {
            Duration::from_secs(2)
        } else {
            Duration::from_secs(15)
        };
        if !probe.wait_ready(timeout, || false, gone) {
            if kill_external_checked(d.pid, d.binary_path.as_deref(), Some(&self.shared.work_dir)) {
                let _ = std::fs::remove_file(Shared::descriptor_path(&self.shared.work_dir));
            }
            return false;
        }
        // Only reuse a snapshot persisted after a successful start/reload and
        // still matching the disk file. Legacy/changed files remain uncached,
        // so rollback never guesses which configuration the live process uses.
        let config = d.verified_config(&self.config_path());
        *self.shared.config_cache.lock().unwrap() = config;
        let mut inner = self.shared.inner.lock().unwrap();
        inner.stopping = false;
        inner.detached = true;
        inner.external_pid = Some(d.pid);
        inner.external_binary = d.binary_path.clone();
        inner.controller = Some((d.port, d.secret));
        inner.status = CoreStatus::Running;
        drop(inner);
        // 收养内核没有子进程句柄：死亡无人知晓，状态会永远停在「运行中」，
        // 系统代理守护也不会触发。起 pid 监视线程补上（身份比对防 pid 复用）。
        #[cfg(any(unix, windows))]
        if let Some(path) = d.binary_path.clone() {
            watch_external(self.shared.clone(), d.pid, path);
        }
        true
    }

    /// Persist a complete handoff before detaching. Failure leaves ownership
    /// intact so an unsuccessful update cannot orphan a live core.
    pub fn prepare_handoff(&self) -> Result<Option<u32>, String> {
        let mut inner = self.shared.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.status != CoreStatus::Running {
            if inner.child.is_some() || inner.external_pid.is_some() {
                return Err("内核尚未就绪，无法安全交接；请稍后重试更新".into());
            }
            return Ok(None);
        }
        let pid = inner
            .child
            .as_ref()
            .map(|child| child.id())
            .or(inner.external_pid)
            .ok_or("运行中的内核缺少进程信息，无法安全交接")?;
        let (port, secret) = inner
            .controller
            .as_ref()
            .ok_or("内核缺少控制器信息，无法安全交接")?;
        let binary = inner
            .external_binary
            .as_deref()
            .unwrap_or(&self.shared.binary_path);
        self.shared
            .write_descriptor(pid, *port, secret, binary)
            .map_err(|error| format!("保存内核交接信息失败：{error}"))?;
        inner.detached = true;
        Ok(Some(pid))
    }

    /// 轻量关窗与自动更新共用同一交接协议。
    pub fn detach(&self) -> Option<u32> {
        self.prepare_handoff().ok().flatten()
    }

    /// 退出失败时重新接管仍存活的内核，让用户可以继续使用或重试退出。
    pub fn stop_for_exit(&self) -> bool {
        let _lifecycle = self
            .shared
            .lifecycle
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let controller = self.controller();
        if self.stop_locked() {
            return true;
        }
        self.resume_after_failed_stop(controller);
        false
    }

    /// 调用方已持有 lifecycle；只探测原进程，不再停止或授权。
    fn resume_after_failed_stop(&self, controller: Option<(u16, String)>) {
        let Some((port, secret)) = controller else {
            return;
        };
        let (pid, binary) = {
            let inner = self.shared.inner.lock().unwrap();
            let Some(pid) = inner.external_pid else {
                return;
            };
            let Some(binary) = inner.external_binary.clone() else {
                // 服务租约缺少身份时保留原状，不能伪造二进制身份。
                return;
            };
            (pid, binary)
        };
        let gone = || !pid_is_alive(pid) || process_matches_binary(pid, &binary) == Some(false);
        let probe = ControllerProbe {
            port,
            secret: secret.clone(),
        };
        if !probe.wait_ready(Duration::from_secs(2), || false, gone) {
            return;
        }
        {
            let mut inner = self.shared.inner.lock().unwrap();
            if inner.external_pid != Some(pid) {
                return;
            }
            inner.controller = Some((port, secret));
            inner.stopping = false;
            inner.detached = false;
        }
        self.shared.notify(CoreStatus::Running);
        #[cfg(any(unix, windows))]
        watch_external(self.shared.clone(), pid, binary);
    }

    /// 优雅停止：标记后杀进程，watch 线程随之退出。
    /// 健康检查触发的内核优雅重启信号（#350）：SIGTERM 不收走句柄、
    /// 不置 stopping——watch 循环检测退出后按崩溃自动重启拉起新内核
    /// （TUN 路由由 mihomo 退出钩子恢复）。复用 tun_health_restart
    /// 15 分钟节流，与 60s 自检共享，避免双重重启竞速。
    #[cfg(unix)]
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    pub fn signal_graceful_restart(&self) -> bool {
        let outcome = {
            let mut inner = self.shared.inner.lock().unwrap_or_else(|e| e.into_inner());
            if inner.stopping || inner.detached {
                return false;
            }
            let Some(child) = inner.child.as_mut() else {
                return false;
            };
            // try_wait 也覆盖 watch 线程已 reap 的退出
            if !matches!(child.try_wait(), Ok(None)) {
                return false;
            }
            let pid = child.id();
            let result = signal_pid(pid, libc::SIGTERM);
            if result.is_ok() {
                // tun_health_restart 仅 macOS 存在（gvisor 栈僵死节流）；
                // Linux 上信号天然不发（服务承载内核无 child 句柄）。
                #[cfg(target_os = "macos")]
                {
                    inner.tun_health_restart = Some((pid, Instant::now()));
                }
                let _ = pid;
            }
            result
        };
        match outcome {
            Ok(()) => true,
            Err(error) => {
                eprintln!("core graceful restart signal failed: {error}");
                false
            }
        }
    }

    #[cfg(not(unix))]
    pub fn signal_graceful_restart(&self) -> bool {
        false // Windows TUN 内核由 SYSTEM 服务承载，停止走服务通道
    }

    pub fn stop(&self) -> bool {
        let _lifecycle = self
            .shared
            .lifecycle
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        self.stop_locked()
    }

    /// 调用方已持有 lifecycle；启动替换 Starting 批次时也走同一条停止路径。
    fn stop_locked(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.shared
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .stopping = true;
        }
        #[cfg(target_os = "linux")]
        match crate::network::linux_service::stop_core() {
            Ok(true) => {
                {
                    let mut inner = self.shared.inner.lock().unwrap_or_else(|e| e.into_inner());
                    inner.stopping = true;
                    inner.external_pid = None;
                    inner.external_binary = None;
                    inner.detached = false;
                    inner.controller = None;
                }
                let _ = std::fs::remove_file(Shared::descriptor_path(&self.shared.work_dir));
                self.shared.notify(CoreStatus::Stopped);
                return true;
            }
            Ok(false) => {}
            Err(error) => {
                self.shared
                    .notify(CoreStatus::Crashed(format!("Linux TUN 清理失败：{error}")));
                return false;
            }
        }
        // Windows TUN 内核以 SYSTEM 运行，GUI 杀不动：先请服务停
        #[cfg(target_os = "windows")]
        {
            if self.shared.request_service_stop().is_ok() {
                {
                    let mut inner = self.shared.inner.lock().unwrap_or_else(|e| e.into_inner());
                    inner.stopping = true;
                    inner.external_pid = None;
                    inner.external_binary = None;
                    inner.detached = false;
                    inner.controller = None;
                }
                let _ = std::fs::remove_file(Shared::descriptor_path(&self.shared.work_dir));
                self.shared.notify(CoreStatus::Stopped);
                return true;
            }
        }
        {
            let mut inner = self.shared.inner.lock().unwrap();
            inner.stopping = true;
            inner.controller = None;
            inner.detached = false;
        }
        let stopped = self.shared.kill_child();
        if stopped {
            self.shared.notify(CoreStatus::Stopped);
        } else {
            self.shared.notify(CoreStatus::Crashed(
                "无法确认内核进程已退出；已保留进程描述以便重试停止".into(),
            ));
        }
        stopped
    }
}

/// Probe the same address the core will use. macOS needs SO_REUSEADDR when a
/// previous listener has just exited but its accepted connections linger.
fn check_mixed_port_available(address: SocketAddr) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use socket2::{Domain, Protocol, Socket, Type};
        let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
        socket.set_reuse_address(true)?;
        socket.bind(&address.into())?;
        socket.listen(1)?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        TcpListener::bind(address).map(|_| ())
    }
}

fn wait_for_mixed_port(
    probe: &impl Fn() -> std::io::Result<()>,
    timeout: Duration,
) -> std::io::Result<()> {
    let deadline = Instant::now() + timeout;
    let mut result = probe();
    while result.is_err() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        result = probe();
    }
    result
}

fn port_has_listener(address: SocketAddr) -> bool {
    let address = if address.ip().is_unspecified() {
        SocketAddr::from(([127, 0, 0, 1], address.port()))
    } else {
        address
    };
    TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_ok()
}

/// 拉起子进程 + 就绪探测 + 监视线程（重启时复用，不重写配置文件）。
fn spawn_and_watch(shared: Arc<Shared>, previous: Option<u32>) {
    match shared.spawn_child(previous) {
        Ok(Some((pid, port, secret))) => {
            if previous.is_some() {
                shared.notify(CoreStatus::Starting);
            }
            crate::lifecycle::authorization::spawn_background(move || {
                watch(shared, pid, port, secret)
            });
        }
        Ok(None) => {} // stop() 或另一条启动链已接管
        Err(error) => shared.notify(CoreStatus::Crashed(error)),
    }
}

/// 活性探测：进程是否存在（kill 0）。
#[cfg(all(unix, test))]
fn kill_pid_probe(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(all(not(unix), test))]
fn kill_pid_probe(_pid: u32) -> bool {
    false
}

const KILL_GRACE: Duration = Duration::from_secs(2);
#[cfg(unix)]
const TERM_GRACE: Duration = Duration::from_secs(5);

fn wait_for_exit(mut exited: impl FnMut() -> bool, grace: Duration) -> bool {
    let deadline = Instant::now() + grace;
    loop {
        if exited() {
            return true;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        std::thread::sleep(remaining.min(Duration::from_millis(50)));
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopOutcome {
    AlreadyGone,
    Terminated,
    Killed,
    PermissionDenied,
    Unconfirmed,
}

#[cfg(unix)]
impl StopOutcome {
    fn confirmed(self) -> bool {
        matches!(self, Self::AlreadyGone | Self::Terminated | Self::Killed)
    }
}

#[cfg(unix)]
fn signal_pid(pid: u32, signal: libc::c_int) -> std::io::Result<()> {
    let pid = i32::try_from(pid)
        .ok()
        .filter(|&pid| pid > 0)
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    if unsafe { libc::kill(pid, signal) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn terminate_unix(
    pid: u32,
    mut exited: impl FnMut() -> bool,
    verified: impl Fn() -> bool,
    term_grace: Duration,
) -> StopOutcome {
    if exited() {
        return StopOutcome::AlreadyGone;
    }
    for (signal, grace, outcome) in [
        (libc::SIGTERM, term_grace, StopOutcome::Terminated),
        (libc::SIGKILL, KILL_GRACE, StopOutcome::Killed),
    ] {
        if exited() {
            return StopOutcome::AlreadyGone;
        }
        if !verified() {
            return if exited() {
                StopOutcome::AlreadyGone
            } else {
                StopOutcome::Unconfirmed
            };
        }
        match signal_pid(pid, signal) {
            Ok(()) => {}
            Err(error) if error.raw_os_error() == Some(libc::EPERM) => {
                return StopOutcome::PermissionDenied;
            }
            Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {
                return if exited() {
                    StopOutcome::AlreadyGone
                } else {
                    StopOutcome::Unconfirmed
                };
            }
            Err(_) => return StopOutcome::Unconfirmed,
        }
        if wait_for_exit(&mut exited, grace) {
            return outcome;
        }
    }
    StopOutcome::Unconfirmed
}

#[cfg(unix)]
pub(crate) fn pid_is_alive(pid: u32) -> bool {
    unsafe {
        let rc = libc::kill(pid as i32, 0);
        rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

#[cfg(windows)]
pub(crate) fn pid_is_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_INVALID_PARAMETER, STILL_ACTIVE,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return GetLastError() != ERROR_INVALID_PARAMETER;
        }
        let mut code = 0;
        let queried = GetExitCodeProcess(process, &mut code);
        CloseHandle(process);
        queried == 0 || code == STILL_ACTIVE as u32
    }
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn pid_is_alive(_pid: u32) -> bool {
    true // 无身份信息时宁可保留描述，也不把 PID 当作安全目标
}

/// AppleScript strings use double quotes; shell quotes do not delimit them.
#[cfg(target_os = "macos")]
fn privileged_shell_script(command: &str) -> String {
    format!(
        "do shell script \"{}\" with administrator privileges",
        command.replace('\\', "\\\\").replace('"', "\\\"")
    )
}

// Shared by the privileged PID and port cleanup paths. The expected executable
// and start time are checked again after the grace period before escalation.
#[cfg(target_os = "macos")]
const ADMIN_STOP_FUNCTION: &str = r#"
process_path() { /bin/ps -p "$1" -o comm=; }
process_start() { LC_ALL=C /bin/ps -p "$1" -o lstart=; }
same_process() {
    [ "$(process_path "$1")" = "$2" ] && [ "$(process_start "$1")" = "$3" ]
}
stop_core() {
    [ -n "$3" ] && same_process "$1" "$2" "$3" || return 1
    /bin/kill -TERM "$1" || return 1
    i=0
    while [ "$i" -lt 20 ]; do
        same_process "$1" "$2" "$3" || { /bin/echo TERM; return 0; }
        /bin/sleep 0.25
        i=$((i + 1))
    done
    same_process "$1" "$2" "$3" || { /bin/echo TERM; return 0; }
    /bin/kill -KILL "$1" || return 1
    i=0
    while [ "$i" -lt 8 ]; do
        same_process "$1" "$2" "$3" || { /bin/echo KILL; return 0; }
        /bin/sleep 0.25
        i=$((i + 1))
    done
    return 1
}
"#;

#[cfg(target_os = "macos")]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(target_os = "macos")]
fn run_admin_stop(script: &str) -> Option<String> {
    crate::lifecycle::authorization::require_interactive().ok()?;
    let out = crate::process::command("/usr/bin/osascript")
        .args(["-e", &privileged_shell_script(script)])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(target_os = "macos")]
fn process_start(pid: u32) -> Option<String> {
    let started = crate::process::command("/bin/ps")
        .env("LC_ALL", "C")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .output()
        .ok()?;
    let value = std::str::from_utf8(&started.stdout)
        .ok()?
        .trim_end_matches('\n');
    (started.status.success() && !value.trim().is_empty()).then(|| value.to_string())
}

#[cfg(target_os = "macos")]
fn escalate_admin_stop(pid: u32, expected: &Path, started: &str) -> Option<StopOutcome> {
    let expected = std::fs::canonicalize(expected).ok()?;
    let script = format!(
        "{ADMIN_STOP_FUNCTION}\nstop_core {pid} {} {}",
        shell_quote(expected.to_str()?),
        shell_quote(started),
    );
    match run_admin_stop(&script).as_deref() {
        Some("TERM") => Some(StopOutcome::Terminated),
        Some("KILL") => Some(StopOutcome::Killed),
        _ => None,
    }
}

/// Stop a verified external process and preserve its descriptor on failure.
fn kill_external_checked(pid: u32, expected: Option<&Path>, work_dir: Option<&Path>) -> bool {
    let Some(expected) = expected else {
        if !pid_is_alive(pid) {
            return true;
        }
        eprintln!("core kill: refusing to signal unverified external pid {pid}");
        return false;
    };
    // A permissions error is not proof that a privileged process has exited.
    #[cfg(target_os = "macos")]
    let started = process_start(pid);
    let gone = || process_matches_binary(pid, expected) == Some(false) || !pid_is_alive(pid);
    if gone() {
        return true;
    }
    if process_matches_binary(pid, expected) != Some(true)
        && !work_dir.is_some_and(|dir| verified_updated_core(pid, expected, dir))
    {
        eprintln!("core kill: cannot verify external process identity; retaining descriptor");
        return false;
    }
    #[cfg(unix)]
    {
        let verified = || {
            #[cfg(target_os = "macos")]
            if started.is_none() || process_start(pid) != started {
                return false;
            }
            process_matches_binary(pid, expected) == Some(true)
                || work_dir.is_some_and(|dir| verified_updated_core(pid, expected, dir))
        };
        let outcome = terminate_unix(pid, gone, verified, TERM_GRACE);
        #[cfg(target_os = "macos")]
        let outcome = if outcome == StopOutcome::PermissionDenied {
            match started
                .as_deref()
                .and_then(|started| escalate_admin_stop(pid, expected, started))
            {
                Some(admin) if wait_for_exit(gone, KILL_GRACE) => admin,
                _ => outcome,
            }
        } else {
            outcome
        };
        if let Some(dir) = work_dir {
            append_decision(
                dir,
                &format!("external core stop: pid={pid} outcome={outcome:?}"),
            );
        }
        outcome.confirmed()
    }
    #[cfg(not(unix))]
    {
        let _ = crate::process::command("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output();
        wait_for_exit(gone, KILL_GRACE)
    }
}

// Tauri's macOS updater moves the running bundle into a temporary directory,
// then deletes it. proc_pidpath returns ENOENT for its still-running sidecar.
// Recover ONLY our persisted PID: a known updater text mapping, ownership of
// the recorded controller listener, and authenticated controller readiness must
// all agree. Never authorize a signal from the executable basename alone.
#[cfg(any(target_os = "macos", test))]
fn updater_text_path(path: &Path, expected: &Path, temporary_root: &Path) -> bool {
    if !expected.ends_with("Crossbow.app/Contents/MacOS/mihomo")
        && !expected.ends_with("Crossbow.app/Contents/MacOS/sing-box")
    {
        return false;
    }
    let Some(name) = expected.file_name() else {
        return false;
    };
    let Ok(relative) = path.strip_prefix(temporary_root) else {
        return false;
    };
    let parts: Vec<_> = relative.components().collect();
    parts.len() == 5
        && parts[0].as_os_str().to_str().is_some_and(|v| {
            v.strip_prefix("tauri_current_app").is_some_and(|suffix| {
                !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_alphanumeric())
            })
        })
        && parts[1].as_os_str() == "current_app"
        && parts[2].as_os_str() == "Contents"
        && parts[3].as_os_str() == "MacOS"
        && parts[4].as_os_str() == name
}

#[cfg(target_os = "macos")]
fn verified_updated_core(pid: u32, expected: &Path, work_dir: &Path) -> bool {
    let Some(d) = Shared::read_descriptor(work_dir) else {
        return false;
    };
    if d.pid != pid || d.binary_path.as_deref() != Some(expected) || d.secret.is_empty() {
        return false;
    }
    let Ok(text) = crate::process::command("/usr/sbin/lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "txt", "-Fn"])
        .output()
    else {
        return false;
    };
    let root = std::fs::canonicalize(std::env::temp_dir()).ok();
    let mapped = String::from_utf8_lossy(&text.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix('n'))
        .any(|name| {
            // The mapped file can be deleted, so normalize only the existing temp root.
            root.as_ref()
                .is_some_and(|root| updater_text_path(Path::new(name), expected, root))
        });
    if !text.status.success() || !mapped {
        return false;
    }
    let Ok(listener) = crate::process::command("/usr/sbin/lsof")
        .args([
            "-nP",
            "-a",
            "-p",
            &pid.to_string(),
            "-i",
            &format!("TCP:{}", d.port),
            "-sTCP:LISTEN",
            "-Fp",
        ])
        .output()
    else {
        return false;
    };
    if !listener.status.success()
        || !String::from_utf8_lossy(&listener.stdout)
            .lines()
            .any(|line| line == format!("p{pid}"))
    {
        return false;
    }
    let probe = ControllerProbe {
        port: d.port,
        secret: d.secret,
    };
    probe.wait_ready(Duration::from_secs(2), || false, || !pid_is_alive(pid))
}
#[cfg(not(target_os = "macos"))]
fn verified_updated_core(_pid: u32, _expected: &Path, _work_dir: &Path) -> bool {
    false
}

#[derive(serde::Deserialize)]
struct CoreDescriptor {
    port: u16,
    secret: String,
    pid: u32,
    #[serde(default)]
    binary_path: Option<PathBuf>,
    #[serde(default)]
    config: Option<String>,
}

impl CoreDescriptor {
    fn verified_config(&self, path: &Path) -> Option<String> {
        let snapshot = self.config.as_ref()?;
        (std::fs::read_to_string(path).ok().as_ref() == Some(snapshot)).then(|| snapshot.clone())
    }
}

/// 枚举可执行路径与任一候选完全一致的进程（识别本应用残留内核，
/// 避免误伤系统里无关的 mihomo/sing-box）。非 macOS 平台返回空——
/// Windows/Linux 的进程路径枚举在各自移植时补齐。
#[cfg(target_os = "macos")]
fn find_leftover_core_pids(candidates: &[PathBuf]) -> Vec<(u32, PathBuf)> {
    use std::ffi::CStr;
    use std::os::raw::{c_int, c_void};

    #[link(name = "proc")]
    unsafe extern "C" {
        fn proc_listallpids(pids: *mut c_void, size: c_int) -> c_int;
        fn proc_pidpath(pid: c_int, buffer: *mut c_void, buffersize: u32) -> c_int;
    }

    let mut out = Vec::new();
    unsafe {
        let n = proc_listallpids(std::ptr::null_mut(), 0);
        if n < 1 {
            return out;
        }
        let mut pids = vec![0_i32; n as usize + 16];
        let n = proc_listallpids(
            pids.as_mut_ptr().cast(),
            (pids.len() * std::mem::size_of::<i32>()) as c_int,
        );
        if n < 1 {
            return out;
        }
        for pid in &pids[..n as usize] {
            let mut buf = [0_i8; 4096];
            let len = proc_pidpath(*pid, buf.as_mut_ptr().cast(), buf.len() as u32);
            if len <= 0 {
                continue;
            }
            let Ok(path) = CStr::from_ptr(buf.as_ptr()).to_str() else {
                continue;
            };
            let path = PathBuf::from(path);
            if candidates.contains(&path) {
                out.push((*pid as u32, path));
            }
        }
    }
    out
}

/// Windows：Toolhelp32 进程快照枚举。快照只给图像名（无完整路径），
/// 因此按「图像文件名 ∈ 候选路径的文件名集合」粗匹配（核心文件名
/// mihomo/sing-box 在 Windows 上不与本应用冲突——本应用侧车为
/// mihomo-x86_64-pc-windows-msvc.exe 全名），完整路径身份校验由
/// kill_external 的 process_matches_binary 二次确认。
#[cfg(target_os = "windows")]
fn find_leftover_core_pids(candidates: &[PathBuf]) -> Vec<(u32, PathBuf)> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let names: std::collections::HashSet<String> = candidates
        .iter()
        .filter_map(|c| c.file_name().map(|n| n.to_string_lossy().to_lowercase()))
        .collect();
    if names.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<(u32, PathBuf)> = Vec::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                let image = String::from_utf16_lossy(
                    &entry.szExeFile[..entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(0)],
                );
                if names.contains(&image.to_lowercase()) {
                    // Pick the actual full-path match, not the first same-named candidate.
                    // kill_external rechecks identity immediately before signalling.
                    if let Some(matched) = candidates.iter().find(|candidate| {
                        process_matches_binary(entry.th32ProcessID, candidate) == Some(true)
                    }) {
                        out.push((entry.th32ProcessID, matched.clone()));
                    }
                }
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
    }
    out
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn find_leftover_core_pids(_candidates: &[PathBuf]) -> Vec<(u32, PathBuf)> {
    Vec::new()
}

/// Windows：解析 netstat -ano 找出端口 LISTENING 的 pid，配 tasklist 取
/// 进程镜像名。内核以管理员/SYSTEM 运行时同样可见（netstat 读内核表）。
#[cfg(target_os = "windows")]
fn windows_port_listeners(port: u16) -> Vec<(u32, String)> {
    let Ok(out) = std::process::Command::new("netstat")
        .args(["-ano", "-p", "tcp"])
        .output()
    else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let needle = format!(":{port} ");
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in text.lines() {
        if !line.to_uppercase().contains("LISTENING") || !line.contains(&needle) {
            continue;
        }
        let Some(last) = line.split_whitespace().last() else {
            continue;
        };
        let Ok(pid) = last.parse::<u32>() else {
            continue;
        };
        if pid == 0 || !seen.insert(pid) {
            continue;
        }
        let image = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        // CSV 形如 "mihomo.exe","1234","Console",... 取第一列去引号
        let image = image
            .split(',')
            .next()
            .unwrap_or("")
            .trim_matches('"')
            .to_string();
        out.push((pid, image));
    }
    out
}

/// The privileged port lookup may see listeners hidden from the GUI user.
/// Only exact executable paths from this installation are eligible.
#[cfg(target_os = "macos")]
fn escalate_clear_port(port: u16, candidates: &[PathBuf], work_dir: &Path) -> bool {
    let paths = candidates
        .iter()
        .filter_map(|path| std::fs::canonicalize(path).ok())
        .filter_map(|path| path.to_str().map(shell_quote))
        .collect::<Vec<_>>();
    if paths.is_empty() {
        return false;
    }
    let script = format!(
        r#"{ADMIN_STOP_FUNCTION}
pids=$(/usr/sbin/lsof -t -nP -i TCP:{port} -sTCP:LISTEN 2>/dev/null)
stopped=0
for p in $pids; do
    case "$p" in ''|*[!0-9]*) continue ;; esac
    actual=$(process_path "$p")
    case "$actual" in
        {paths})
            started=$(process_start "$p")
            /usr/sbin/lsof -t -nP -a -p "$p" -i TCP:{port} -sTCP:LISTEN 2>/dev/null | /usr/bin/grep -Fxq "$p" || continue
            stop_core "$p" "$actual" "$started" && stopped=1
            ;;
    esac
done
test "$stopped" -eq 1
"#,
        paths = paths.join("|")
    );
    let result = run_admin_stop(&script);
    if let Some(outcomes) = &result {
        for outcome in outcomes
            .lines()
            .filter(|line| matches!(*line, "TERM" | "KILL"))
        {
            append_decision(
                work_dir,
                &format!("port cleanup: port={port} outcome={outcome}"),
            );
        }
    }
    result.is_some()
}

#[cfg(not(target_os = "macos"))]
fn escalate_clear_port(_port: u16, _candidates: &[PathBuf], _work_dir: &Path) -> bool {
    false
}

/// lsof 列出端口监听者的 (pid, 命令行)——仅当前用户可见的范围；
/// root 进程对普通权限不可见（由 escalate_clear_port 兜底）。
#[cfg(unix)]
fn port_listeners_via_lsof(port: u16) -> Vec<(u32, String)> {
    let Ok(out) = crate::process::command("lsof")
        .args(["-n", "-P", "-i", &format!("TCP:{port}"), "-sTCP:LISTEN"])
        .output()
    else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() > 1 {
            if let Ok(pid) = cols[1].parse::<u32>() {
                let cmd = crate::process::command("ps")
                    .args(["-p", &pid.to_string(), "-o", "command="])
                    .output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .unwrap_or_default();
                out.push((pid, cmd));
            }
        }
    }
    out
}

#[cfg(not(unix))]
fn port_listeners_via_lsof(_port: u16) -> Vec<(u32, String)> {
    Vec::new()
}

#[cfg(target_os = "macos")]
fn process_matches_binary(pid: u32, expected: &Path) -> Option<bool> {
    use std::ffi::CStr;
    use std::os::raw::{c_int, c_void};

    #[link(name = "proc")]
    unsafe extern "C" {
        fn proc_pidpath(pid: c_int, buffer: *mut c_void, buffersize: u32) -> c_int;
    }

    let mut buffer = [0_i8; 4096];
    let len = unsafe {
        proc_pidpath(
            pid as c_int,
            buffer.as_mut_ptr().cast(),
            buffer.len() as u32,
        )
    };
    if len <= 0 {
        return None;
    }
    let Ok(actual) = unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_str() else {
        return None;
    };
    match (
        std::fs::canonicalize(actual),
        std::fs::canonicalize(expected),
    ) {
        (Ok(actual), Ok(expected)) => Some(actual == expected),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn process_matches_binary(pid: u32, expected: &Path) -> Option<bool> {
    match (
        std::fs::canonicalize(format!("/proc/{pid}/exe")),
        std::fs::canonicalize(expected),
    ) {
        (Ok(actual), Ok(expected)) => Some(actual == expected),
        _ => None,
    }
}

#[cfg(target_os = "windows")]
fn process_matches_binary(pid: u32, expected: &Path) -> Option<bool> {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_INVALID_PARAMETER};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return if GetLastError() == ERROR_INVALID_PARAMETER {
                Some(false)
            } else {
                None
            };
        }
        let mut buf = [0_u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
        CloseHandle(handle);
        if ok == 0 {
            return None;
        }
        let actual = String::from_utf16_lossy(&buf[..len as usize]);
        // Windows 路径大小写不敏感；canonicalize 失败时退化为不区分大小写的直比
        match (
            std::fs::canonicalize(&actual),
            std::fs::canonicalize(expected),
        ) {
            (Ok(actual), Ok(expected)) => Some(
                actual
                    .as_os_str()
                    .eq_ignore_ascii_case(expected.as_os_str()),
            ),
            _ => Some(actual.eq_ignore_ascii_case(&expected.to_string_lossy())),
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn process_matches_binary(_pid: u32, _expected: &Path) -> Option<bool> {
    None
}

impl Shared {
    fn read_descriptor(work_dir: &Path) -> Option<CoreDescriptor> {
        let raw = std::fs::read_to_string(Self::descriptor_path(work_dir)).ok()?;
        serde_json::from_str(&raw).ok()
    }
}

/// 收养内核的 pid 监视：每 3 秒做一次二进制路径身份比对，进程消失且
/// 收养角色未变时上报 Crashed（联动系统代理还原、托盘退订等守护）。
/// stop()/重新 start() 会先解除收养角色（external_pid 置空/换新），
/// 监视线程据此静默退场，不会误报。身份查询失败时保留存活状态，
/// 不把权限不足当作退出；Windows 使用原生进程存活查询。
#[cfg(any(unix, windows))]
fn watch_external(shared: Arc<Shared>, pid: u32, binary: PathBuf) {
    crate::lifecycle::authorization::spawn_background(move || loop {
        std::thread::sleep(Duration::from_secs(3));
        {
            let inner = shared.inner.lock().unwrap_or_else(|e| e.into_inner());
            if inner.stopping || inner.external_pid != Some(pid) {
                return;
            }
        }
        if process_matches_binary(pid, &binary) != Some(false) && pid_is_alive(pid) {
            continue; // Permissions may temporarily prevent inspecting a root core.
        }
        let mut inner = shared.inner.lock().unwrap();
        if inner.stopping || inner.external_pid != Some(pid) {
            return; // 已被 stop/接管/换核：收尾由对方负责
        }
        inner.external_pid = None;
        inner.detached = false;
        inner.controller = None;
        drop(inner);
        append_decision(
            &shared.work_dir,
            &format!("adopted-watch: external pid {pid} gone → Crashed"),
        );
        shared.notify(CoreStatus::Crashed(
            "收养的内核进程已退出（可能是被外部终止或异常崩溃）".into(),
        ));
        return;
    });
}

/// A live service connection confirms both core lifetime and owned resources.
#[cfg(target_os = "linux")]
fn watch_linux_service(shared: Arc<Shared>, pid: u32) {
    crate::lifecycle::authorization::spawn_background(move || loop {
        std::thread::sleep(Duration::from_secs(1));
        {
            let inner = shared.inner.lock().unwrap_or_else(|e| e.into_inner());
            if inner.stopping || inner.external_pid != Some(pid) {
                return;
            }
        }
        let (result, cleanup_confirmed) = crate::network::linux_service::watch_status(pid);
        if result
            .as_ref()
            .ok()
            .and_then(|s| s.as_ref())
            .is_some_and(|s| s.pid == pid && s.active)
        {
            continue;
        }
        let message = match result {
            Ok(None) => "Linux TUN 服务已停止，接管资源已释放".to_string(),
            Ok(Some(_)) => "Linux TUN 服务状态异常，尚未确认清理完成".to_string(),
            Err(error) => {
                // A terminal failure can confirm cleanup and still return its cause.
                if cleanup_confirmed {
                    format!("Linux TUN 服务已停止，接管资源已释放：{error}")
                } else {
                    format!("Linux TUN 服务异常，尚未确认清理完成：{error}")
                }
            }
        };
        {
            let inner = shared.inner.lock().unwrap_or_else(|e| e.into_inner());
            if inner.stopping || inner.external_pid != Some(pid) {
                return;
            }
        }
        // Keep the tracked PID on uncertain cleanup so a later stop can retry.
        shared.notify(CoreStatus::Crashed(message));
        return;
    });
}

/// 服务托管内核的就绪探测（watch() 的无子进程变体）：置 Running 或 Crashed。
fn probe_external_core(shared: Arc<Shared>, pid: u32, port: u16, secret: String) {
    let timeout = Duration::from_secs(shared.opts.lock().unwrap().readiness_timeout_secs);
    let probe = ControllerProbe { port, secret };
    let ready = probe.wait_ready(
        timeout,
        || !shared.external_start_is_current(pid, &probe),
        || false,
    );
    finish_external_probe(&shared, pid, &probe, ready);
}

/// 等到 lifecycle 后再次核对本批次。探测取消、HTTP 迟到和服务停止回包
/// 都不能跨过 stop/start 接管下一批次；通知也必须在这次交接完成前发出。
fn finish_external_probe(shared: &Shared, pid: u32, probe: &ControllerProbe, ready: bool) {
    let _lifecycle = shared.lifecycle.lock().unwrap_or_else(|e| e.into_inner());
    if !shared.external_start_is_current(pid, probe) {
        return;
    }
    if ready {
        shared.notify(CoreStatus::Running);
    } else {
        // 超时后先请服务收走仍在跑的内核。收不走就留下租约，下次启动走同一条清理。
        let _ = shared.release_hosted_before_kill();
        shared.notify(CoreStatus::Crashed("服务托管内核启动失败/超时".into()));
    }
}

fn watch(shared: Arc<Shared>, pid: u32, port: u16, secret: String) {
    let opts = shared.opts.lock().unwrap().clone();

    // 首次启动：GeoIP 数据库缺失/过小时先从镜像下载（mihomo 自带的
    // 直连 GitHub 下载在国内网络极易超时，且被旧版就绪窗口误杀形成
    // 「半截文件」死循环）。
    if opts.ensure_geo_files {
        if let Err(e) = ensure_geo_files(&shared.work_dir) {
            shared.kill_child();
            shared.notify(CoreStatus::Crashed(format!("GeoIP 数据库准备失败：{e}")));
            return;
        }
    }

    let probe = ControllerProbe { port, secret };
    // 子进程在探测窗口内退出（如端口被占 FATAL）→ 立刻失败，不傻等满超时
    let child_gone = || {
        let mut inner = shared.inner.lock().unwrap();
        match inner.child.as_mut() {
            Some(c) if c.id() == pid => c.try_wait().ok().flatten().is_some(),
            _ => true,
        }
    };
    if !probe.wait_ready(
        Duration::from_secs(opts.readiness_timeout_secs),
        || shared.inner.lock().unwrap().stopping,
        child_gone,
    ) {
        let stopping = {
            let inner = shared.inner.lock().unwrap();
            inner.stopping || inner.child.as_ref().map(Child::id) != Some(pid)
        };
        if stopping {
            return;
        }
        // 无论打断原因，先收走子进程（stop 打断路径若不杀，刚拉起的内核会变孤儿）
        shared.kill_child();
        let summary = core_error_summary(&shared.work_dir, 200).unwrap_or_else(|| {
            format!(
                "就绪探针 {}s 未通过，内核无错误输出（详见监测→日志）",
                opts.readiness_timeout_secs
            )
        });
        shared.notify(CoreStatus::Crashed(format!("内核启动失败/超时：{summary}")));
        return;
    }
    {
        let inner = shared.inner.lock().unwrap();
        if inner.stopping || inner.child.as_ref().map(Child::id) != Some(pid) {
            return;
        }
    }
    shared.notify(CoreStatus::Running);
    #[cfg(target_os = "macos")]
    if let Some(mixed_port) = shared
        .config_cache
        .lock()
        .unwrap()
        .as_deref()
        .and_then(tun_health_port)
    {
        let weak = Arc::downgrade(&shared);
        crate::lifecycle::authorization::spawn_background(move || {
            tun_health_monitor(weak, pid, mixed_port)
        });
    }

    // 子进程始终留在共享状态（stop/pid 都依赖它）；这里只轮询退出状态。
    loop {
        let exit = {
            let mut inner = shared.inner.lock().unwrap();
            match inner.child.as_mut() {
                Some(c) if c.id() == pid => c.try_wait().ok().flatten(),
                _ => return, // 已停止或换核
            }
        };
        let Some(exit) = exit else {
            std::thread::sleep(Duration::from_millis(200));
            continue;
        };
        if shared.inner.lock().unwrap().stopping {
            return; // stop() 负责上报 Stopped
        }
        #[cfg(target_os = "macos")]
        let planned = shared
            .inner
            .lock()
            .unwrap()
            .tun_health_restart
            .is_some_and(|(target, _)| target == pid)
            && exit.code() == Some(0);
        #[cfg(not(target_os = "macos"))]
        let planned = false;
        let restart = allow_restart(&shared);
        if planned && restart {
            append_decision(
                &shared.work_dir,
                &format!("tun health: pid={pid} exited cleanly; restarting"),
            );
        } else {
            let summary = core_error_summary(&shared.work_dir, 200)
                .unwrap_or_else(|| "内核无错误输出（详见监测→日志）".into());
            let reason = match exit.code() {
                Some(code) => format!("内核异常退出（code {code}）：{summary}"),
                None => format!("内核被信号终止：{summary}"),
            };
            shared.notify(CoreStatus::Crashed(reason));
        }

        // 崩溃或自检触发的正常退出都使用既有重启预算。
        if restart {
            std::thread::sleep(Duration::from_millis(800));
            spawn_and_watch(shared, Some(pid));
        }
        return;
    }
}

/// 崩溃重启簿记：窗口内超次返回 false（保持 Crashed）。
fn allow_restart(shared: &Shared) -> bool {
    let opts = shared.opts.lock().unwrap().clone();
    let mut inner = shared.inner.lock().unwrap();
    let now = Instant::now();
    while inner
        .restarts
        .front()
        .is_some_and(|t| now.duration_since(*t).as_secs() > opts.restart_window_secs)
    {
        inner.restarts.pop_front();
    }
    if inner.restarts.len() as u32 >= opts.max_restarts {
        return false;
    }
    inner.restarts.push_back(now);
    true
}

struct ControllerProbe {
    port: u16,
    secret: String,
}

impl ControllerProbe {
    /// 轮询 `/version` 直到就绪或超时；`cancelled` 返回 true 时立即放弃
    /// （stop() 已接管收尾，本探测不再上报）。
    fn wait_ready(
        &self,
        timeout: Duration,
        cancelled: impl Fn() -> bool,
        child_gone: impl Fn() -> bool,
    ) -> bool {
        let agent = ureq::AgentBuilder::new()
            .try_proxy_from_env(false)
            .timeout(Duration::from_millis(800))
            .build();
        let url = format!("http://127.0.0.1:{}/version", self.port);
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if cancelled() || child_gone() {
                return false;
            }
            if let Ok(resp) = agent
                .get(&url)
                .set("Authorization", &format!("Bearer {}", self.secret))
                .call()
            {
                if resp.status() == 200 {
                    return true;
                }
            }
            // 短睡眠分片，保证 stop() 能及时打断长探测
            for _ in 0..6 {
                if cancelled() || child_gone() {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        false
    }
}

/// GeoIP/GeoSite 数据源：镜像优先，直连 GitHub 兜底。
/// 镜像失效时调整此列表即可（均为数据文件，不参与代码构建）。
pub const GEO_FILES: &[(&str, bool, u64, &[&str])] = &[
    (
        "geoip.metadb",
        true,
        4_000_000, // 完整 metadb ~8-9MB；4MB 下限过滤弱网截断残片
        &[
            // 2026-10 在 CN 网络实测可用；jsDelivr 为 CDN 备源
            "https://gh-proxy.com/https://github.com/MetaCubeX/meta-rules-dat/releases/latest/download/geoip.metadb",
            "https://cdn.jsdelivr.net/gh/MetaCubeX/meta-rules-dat@meta/geoip.metadb",
            "https://github.com/MetaCubeX/meta-rules-dat/releases/latest/download/geoip.metadb",
        ],
    ),
    (
        "geosite.dat",
        false, // 尽力而为：仅当规则用到 GEOSITE 时内核才需要
        500_000,
        &[
            "https://gh-proxy.com/https://github.com/MetaCubeX/meta-rules-dat/releases/latest/download/geosite.dat",
            "https://cdn.jsdelivr.net/gh/MetaCubeX/meta-rules-dat@meta/geosite.dat",
            "https://github.com/MetaCubeX/meta-rules-dat/releases/latest/download/geosite.dat",
        ],
    ),
];

/// 确保内核工作目录里有可用的 geo 数据文件；缺失/过小时逐镜像下载。
fn ensure_geo_files(work_dir: &std::path::Path) -> Result<(), String> {
    ensure_geo_files_with(work_dir, GEO_FILES, download_to)
}

fn ensure_geo_files_with(
    work_dir: &std::path::Path,
    files: &[(&str, bool, u64, &[&str])],
    download: impl Fn(&str, &std::path::Path) -> Result<(), String>,
) -> Result<(), String> {
    for (name, required, min_bytes, _) in files {
        let dest = work_dir.join(name);
        // 已存在的文件也要过健全性检查：弱网截断的残片（≥旧阈值但内容不完整）
        // 会让内核报「MMDB invalid」并进入自身下载死循环——实测踩坑（新用户首启）。
        if dest.exists() && geo_file_ok(name, &dest, *min_bytes) {
            continue;
        }
        match download(name, &dest) {
            Ok(()) => {}
            Err(e) if !required => {
                // 尽力而为的文件（geosite）：失败时清掉残留的坏文件——
                // 留着会让内核解析规则时直接失败；缺失时内核会自行下载。
                let _ = std::fs::remove_file(&dest);
                append_core_log(
                    work_dir,
                    &format!("{name} download failed (non-fatal): {e}"),
                );
            }
            Err(e) => {
                return Err(format!(
                    "{name}: {e}。若多次失败：可临时用手机热点完成首次下载，或从其他可用设备复制该文件到 runtime 目录"
                ))
            }
        }
    }
    Ok(())
}

/// 下载/已存文件的健全性检查：大小下限；metadb 额外要求文件尾部存在
/// MMDB metadata 标志串（\xab\xcd\xef"MaxMind.com"）——弱网中断的截断残片
/// 会丢失尾部标志，这正是内核报「MMDB invalid」并进入自身下载死循环的
/// 形态（新用户首启实测踩坑）。
fn geo_file_ok(name: &str, path: &std::path::Path, min_bytes: u64) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if meta.len() < min_bytes {
        return false;
    }
    if !name.ends_with("metadb") {
        return true; // geosite.dat：protobuf 容器，无可靠尾部标志，靠大小下限
    }
    const MARKER: &[u8] = b"\xab\xcd\xefMaxMind.com";
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    use std::io::{Read, Seek, SeekFrom};
    let start = meta.len().saturating_sub(64 * 1024);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return false;
    }
    let mut buf = Vec::with_capacity((meta.len() - start) as usize);
    if f.read_to_end(&mut buf).is_err() {
        return false;
    }
    buf.windows(MARKER.len()).any(|w| w == MARKER)
}

/// 依次尝试 GeoFiles 里该文件的镜像源；下载后做健全性检查（大小 + metadb 尾部标志）。
fn download_to(name: &str, dest: &std::path::Path) -> Result<(), String> {
    let (urls, min_bytes) = GEO_FILES
        .iter()
        .find(|(n, _, _, _)| *n == name)
        .map(|(_, _, min, urls)| (*urls, *min))
        .ok_or_else(|| format!("unknown geo file {name}"))?;
    fetch_first_ok(name, urls, dest, min_bytes)
}

/// 依次尝试镜像源下载；下载后做最小健全性检查（大小下限；metadb 另查文件头）。
fn fetch_first_ok(
    name: &str,
    urls: &[&str],
    dest: &std::path::Path,
    min_bytes: u64,
) -> Result<(), String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(90))
        .build();
    let mut last_err = String::from("no source attempted");
    for url in urls {
        append_core_log(
            dest.parent().unwrap_or(std::path::Path::new(".")),
            &format!("Downloading {name} from {url}"),
        );
        match agent.get(url).call() {
            Ok(resp) => {
                if resp.status() >= 400 {
                    last_err = format!("{url}: HTTP {}", resp.status());
                    continue;
                }
                let mut reader = resp.into_reader();
                let tmp = dest.with_extension("part");
                let mut w = std::io::BufWriter::new(
                    std::fs::File::create(&tmp).map_err(|e| e.to_string())?,
                );
                if let Err(e) = std::io::copy(&mut reader, &mut w) {
                    last_err = format!("{url}: {e}");
                    let _ = std::fs::remove_file(&tmp);
                    continue;
                }
                if !geo_file_ok(name, &tmp, min_bytes) {
                    last_err = format!("{url}: downloaded file failed sanity check");
                    let _ = std::fs::remove_file(&tmp);
                    continue;
                }
                std::fs::rename(&tmp, dest).map_err(|e| e.to_string())?;
                append_core_log(
                    dest.parent().unwrap_or(std::path::Path::new(".")),
                    &format!(
                        "Downloaded {name} ({} bytes)",
                        dest.metadata().map(|m| m.len()).unwrap_or(0)
                    ),
                );
                return Ok(());
            }
            Err(e) => {
                last_err = format!("{url}: {e}");
            }
        }
    }
    Err(format!("所有源均失败：{last_err}"))
}

/// 启动链决策日志：独立追加文件，spawn 的日志截断碰不到它。
/// 每行带 epoch 毫秒，用于离线还原「谁在什么时候用哪个二进制/配置启动」。
pub fn append_decision(work_dir: &std::path::Path, line: &str) {
    use std::io::Write;
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(work_dir.join("core-decision.log"))
    {
        let _ = writeln!(f, "{ms} {line}");
    }
}

fn append_core_log(work_dir: &std::path::Path, line: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(work_dir.join("core-stdio.log"))
    {
        let _ = writeln!(f, "[crossbow] {line}");
    }
}

/// Only probe mihomo's gVisor automatic routing setup. A TUN setting in an
/// unrelated YAML block (or a sing-box JSON profile) must not start this worker.
#[cfg(target_os = "macos")]
fn tun_health_port(cfg: &str) -> Option<u16> {
    let cfg: serde_yaml::Value = serde_yaml::from_str(cfg).ok()?;
    let tun = &cfg["tun"];
    if tun["enable"].as_bool() != Some(true)
        || tun["auto-route"].as_bool() != Some(true)
        || tun["stack"].as_str() != Some("gvisor")
    {
        return None;
    }
    u16::try_from(cfg["mixed-port"].as_u64()?)
        .ok()
        .filter(|&port| port != 0)
}

#[cfg(target_os = "macos")]
const TUN_PROBE_HOSTS: [&str; 2] = ["1.1.1.1", "8.8.8.8"];

#[cfg(target_os = "macos")]
fn route_fields(host: &str) -> Option<(String, String)> {
    let output = crate::process::command("/sbin/route")
        .args(["-n", "get", host])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = std::str::from_utf8(&output.stdout).ok()?;
    let field = |name| {
        text.lines()
            .find_map(|line| line.trim().strip_prefix(name).map(str::trim))
    };
    Some((
        field("interface:")?.to_string(),
        field("gateway:").unwrap_or("").to_string(),
    ))
}

#[cfg(target_os = "macos")]
fn capture_interface(host: &str) -> Option<String> {
    let (interface, gateway) = route_fields(host)?;
    let unit = interface.strip_prefix("utun")?;
    (!unit.is_empty() && unit.bytes().all(|c| c.is_ascii_digit()) && gateway == "198.18.0.1")
        .then_some(interface)
}

#[cfg(target_os = "macos")]
pub(crate) fn tun_route_diagnostics() -> String {
    let default = route_fields("default")
        .map(|(interface, _)| interface)
        .unwrap_or_else(|| "unavailable".into());
    let capture = capture_interface(TUN_PROBE_HOSTS[0]).unwrap_or_else(|| "none/unknown".into());
    format!("default_interface={default} sample_capture_interface={capture}")
}

#[cfg(target_os = "macos")]
fn logged_tun_interface(log: &str) -> Option<&str> {
    log.lines().rev().find_map(|line| {
        let (_, adapter) = line.split_once("Tun adapter listening at: ")?;
        let (interface, address) = adapter.split_once('(')?;
        let unit = interface.strip_prefix("utun")?;
        (!unit.is_empty()
            && unit.bytes().all(|c| c.is_ascii_digit())
            && address.starts_with("[198.18.0.1/"))
        .then_some(interface)
    })
}

#[cfg(target_os = "macos")]
fn tun_path_differs(mixed_port: u16, own_interface: &str) -> bool {
    let Ok(proxy) = ureq::Proxy::new(format!("http://127.0.0.1:{mixed_port}")) else {
        return false;
    };
    let builder = || {
        ureq::AgentBuilder::new()
            .try_proxy_from_env(false)
            .timeout(Duration::from_secs(4))
            .redirects(0)
    };
    let direct = builder().build();
    let mixed = builder().proxy(proxy).build();
    let reachable = |agent: &ureq::Agent, url: &str| {
        matches!(
            agent.head(url).call(),
            Ok(_) | Err(ureq::Error::Status(_, _))
        )
    };
    TUN_PROBE_HOSTS.iter().all(|host| {
        let routed_here = || capture_interface(host).as_deref() == Some(own_interface);
        let url = format!("https://{host}/");
        routed_here() && !reachable(&direct, &url) && reachable(&mixed, &url) && routed_here()
    })
}

#[cfg(target_os = "macos")]
fn current_tun_child(inner: &Inner, pid: u32) -> bool {
    !inner.stopping
        && !inner.detached
        && inner.status == CoreStatus::Running
        && inner.child.as_ref().map(Child::id) == Some(pid)
}

#[cfg(target_os = "macos")]
fn tun_health_monitor(weak: std::sync::Weak<Shared>, pid: u32, mixed_port: u16) {
    use std::io::Read;
    let mut failures = 0;
    loop {
        std::thread::sleep(Duration::from_secs(60));
        let Some(shared) = weak.upgrade() else {
            return;
        };
        if !current_tun_child(&shared.inner.lock().unwrap(), pid) {
            return;
        }

        // Both endpoints must fail only over the captured path. Outages that
        // affect the mixed port too, network changes and missing routes reset
        // the streak rather than causing a restart.
        let mut startup_log = Vec::new();
        let differs = std::fs::File::open(shared.work_dir.join("core-stdio.log"))
            .and_then(|file| file.take(128 * 1024).read_to_end(&mut startup_log))
            .is_ok()
            && logged_tun_interface(&String::from_utf8_lossy(&startup_log))
                .is_some_and(|interface| tun_path_differs(mixed_port, interface));
        failures = if differs { failures + 1 } else { 0 };
        if failures < 3 {
            continue;
        }
        failures = 0;

        let outcome = {
            let mut inner = shared.inner.lock().unwrap();
            if !current_tun_child(&inner, pid) {
                return;
            }
            if inner
                .tun_health_restart
                .is_some_and(|(_, when)| when.elapsed() < Duration::from_secs(900))
            {
                continue;
            }
            // try_wait also catches an exit that the watcher has already reaped.
            let Some(child) = inner.child.as_mut() else {
                return;
            };
            if !matches!(child.try_wait(), Ok(None)) {
                return;
            }
            // Hold the ownership lock through the signal so a manual stop or
            // handoff cannot race a probe result. Never force-kill automatically.
            let result = signal_pid(pid, libc::SIGTERM);
            if result.is_ok() {
                inner.tun_health_restart = Some((pid, Instant::now()));
            }
            result
        };
        match outcome {
            Ok(()) => {
                append_decision(
                    &shared.work_dir,
                    &format!("tun health: pid={pid} capture probes failed; sent SIGTERM"),
                );
                drop(shared);
                std::thread::sleep(TERM_GRACE);
                if let Some(shared) = weak.upgrade() {
                    if current_tun_child(&shared.inner.lock().unwrap(), pid) {
                        append_decision(
                            &shared.work_dir,
                            &format!(
                                "tun health: pid={pid} did not exit in time; no automatic SIGKILL"
                            ),
                        );
                    }
                }
                return;
            }
            Err(error) => {
                append_decision(
                    &shared.work_dir,
                    &format!("tun health: pid={pid} SIGTERM failed: {error}"),
                );
                return;
            }
        }
    }
}

/// 判断内核日志行是否为错误/致命级别
/// （mihomo logfmt、sing-box JSON、大写 FATAL、panic）。
fn is_error_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("level=error")
        || lower.contains("level=fatal")
        || lower.contains("\"level\":\"error\"")
        || lower.contains("\"level\":\"fatal\"")
        || lower.contains("fatal")
        || lower.contains("panic")
}

/// 提炼单行摘要：logfmt 行取 msg 内容，去掉 time=/level= 前缀，
/// 压平空白并截断。绝不保留换行——Crashed 消息会流进首页状态徽标。
fn sanitize_log_line(line: &str, max_chars: usize) -> String {
    let mut owned = line.trim().to_string();
    if let Some(start) = owned.find("msg=\"") {
        let rest = owned[start + 5..].to_string();
        if let Some(end) = rest.rfind('"') {
            owned = rest[..end].trim().to_string();
        }
    } else {
        let kept: Vec<&str> = owned
            .split_whitespace()
            .filter(|token| {
                let lower = token.to_ascii_lowercase();
                !lower.starts_with("time=") && !lower.starts_with("level=")
            })
            .collect();
        owned = kept.join(" ");
    }
    let mut out = owned.split_whitespace().collect::<Vec<&str>>().join(" ");
    if out.chars().count() > max_chars {
        out = out.chars().take(max_chars).collect::<String>() + "…";
    }
    out
}

/// 崩溃/超时原因摘要：只取内核输出里最后一条错误/致命级行，
/// 单行、截断。找不到错误行时返回 None（调用方给通用文案）——
/// 绝不把 info 级连接日志原文塞进 UI（#250）。
fn core_error_summary(work_dir: &std::path::Path, max_chars: usize) -> Option<String> {
    let raw = std::fs::read_to_string(work_dir.join("core-stdio.log")).ok()?;
    let line = raw
        .lines()
        .rev()
        .filter(|l| !l.trim().is_empty())
        .find(|l| is_error_line(l))?;
    Some(sanitize_log_line(line, max_chars))
}

/// 分配一个空闲 TCP 端口（先绑定 0 拿端口再释放，存在极小竞态，S2 可接受）。
pub fn free_port() -> Option<u16> {
    let l = TcpListener::bind(("127.0.0.1", 0)).ok()?;
    l.local_addr().ok().map(|a| a.port())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    /// 状态回调不可占着回调注册锁，退出和崩溃恢复可能同时通知。
    #[test]
    fn callback_runs_without_holding_registration_lock() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let dir = tempfile::tempdir().unwrap();
        let manager =
            super::CoreManager::new(std::path::PathBuf::from("unused-core"), dir.path().into());
        let shared = manager.shared.clone();
        let weak = std::sync::Arc::downgrade(&shared);
        let unlocked = std::sync::Arc::new(AtomicBool::new(false));
        let observed = unlocked.clone();
        manager.set_callback(std::sync::Arc::new(move |_| {
            observed.store(
                weak.upgrade().unwrap().callback.try_lock().is_ok(),
                Ordering::SeqCst,
            );
        }));
        shared.notify(super::CoreStatus::Stopped);
        shared.callback.lock().unwrap().take();
        assert!(unlocked.load(Ordering::SeqCst));
    }

    #[test]
    fn error_summary_picks_last_error_line_and_extracts_msg() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("core-stdio.log");
        std::fs::write(
            &log,
            concat!(
                "2026-11-01T12:25:33+08:00 level=info msg=\"[TCP] 127.0.0.1:44683(TslGame.exe) --> a:443\"\n",
                "time=\"2026-11-01T12:26:00+08:00\" level=error msg=\"Start Configuration error\"\n",
                "level=info msg=\"[TCP] more traffic\"\n",
            ),
        )
        .unwrap();
        let s = core_error_summary(dir.path(), 200).unwrap();
        assert_eq!(s, "Start Configuration error");
    }

    #[test]
    fn error_summary_none_when_only_info_lines() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("core-stdio.log"),
            "level=info msg=\"[TCP] traffic\"\n",
        )
        .unwrap();
        assert!(core_error_summary(dir.path(), 200).is_none());
    }

    #[test]
    fn error_summary_matches_fatal_and_json_levels() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("core-stdio.log"),
            "FATAL init config: yaml unmarshal errors\n",
        )
        .unwrap();
        let s = core_error_summary(dir.path(), 200).unwrap();
        assert!(s.starts_with("FATAL init config"));
        let dir2 = tempfile::tempdir().unwrap();
        std::fs::write(
            dir2.path().join("core-stdio.log"),
            "{\"level\":\"fatal\",\"msg\":\"service start failed\"}\n",
        )
        .unwrap();
        assert!(core_error_summary(dir2.path(), 200).is_some());
    }

    #[test]
    fn sanitize_flattens_newlines_and_truncates() {
        let out = sanitize_log_line("FATAL\ninit config:\n  bad yaml", 10);
        assert!(!out.contains('\n'));
        assert!(out.chars().count() <= 11);
        assert!(out.ends_with('…'));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn tun_probe_requires_mihomo_gvisor_and_auto_route() {
        let cfg = "mixed-port: 7898\ntun: {enable: true, stack: gvisor, auto-route: true}";
        assert_eq!(tun_health_port(cfg), Some(7898));
        assert_eq!(
            tun_health_port(&cfg.replace("enable: true", "enable: false")),
            None
        );
        assert_eq!(
            tun_health_port(&cfg.replace("auto-route: true", "auto-route: false")),
            None
        );
        assert_eq!(tun_health_port(&cfg.replace("gvisor", "system")), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn tun_probe_uses_only_the_adapter_reported_by_its_core() {
        let log = "level=info msg=\"[TUN] Tun adapter listening at: utun8([198.18.0.1/30]), stack: gvisor\"";
        assert_eq!(logged_tun_interface(log), Some("utun8"));
        assert_eq!(
            logged_tun_interface(&log.replace("198.18.0.1", "10.0.0.1")),
            None
        );
        assert_eq!(logged_tun_interface(&log.replace("utun8", "en0")), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn admin_stop_script_rechecks_process_start_before_signalling() {
        let mut child = Command::new("/bin/sleep").arg("10").spawn().unwrap();
        let pid = child.id();
        let started = process_start(pid).unwrap();
        let script = |started: &str| {
            format!(
                "{ADMIN_STOP_FUNCTION}\nstop_core {pid} /bin/sleep {}",
                shell_quote(started),
            )
        };
        let wrong = Command::new("/bin/sh")
            .args(["-c", &script("stale")])
            .output()
            .unwrap();
        assert!(!wrong.status.success());
        assert!(child.try_wait().unwrap().is_none());

        // The script's success check waits for disappearance. Reap the
        // child concurrently, as launchd would for an orphaned core.
        let reaper = std::thread::spawn(move || child.wait().unwrap());
        let correct = Command::new("/bin/sh")
            .args(["-c", &script(&started)])
            .output()
            .unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(reaper.join().unwrap().signal(), Some(libc::SIGTERM));
        assert!(correct.status.success());
        assert_eq!(String::from_utf8_lossy(&correct.stdout).trim(), "TERM");
    }

    #[test]
    fn stale_restart_cannot_spawn_after_stop_or_replace_another_core() {
        let dir = tempfile::tempdir().unwrap();
        let manager = CoreManager::new(PathBuf::from("should-not-be-launched"), dir.path().into());
        {
            let mut inner = manager.shared.inner.lock().unwrap();
            inner.controller = Some((12345, String::new()));
            inner.stopping = true;
        }
        assert!(manager.shared.spawn_child(None).unwrap().is_none());
        manager.shared.inner.lock().unwrap().stopping = false;
        assert!(manager.shared.spawn_child(Some(1)).unwrap().is_none());
        assert!(!manager.config_path().exists());
    }

    #[cfg(unix)]
    fn test_stop_child(trap: &str) -> (StopOutcome, std::process::ExitStatus) {
        use std::io::{BufRead, BufReader};
        let mut child = Command::new("/bin/sh")
            .args([
                "-c",
                &format!("{trap}; echo ready; while :; do /bin/sleep 0.05; done"),
            ])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        // Synchronize with the installed handler instead of signalling a shell
        // that may not have reached trap yet.
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "ready");
        let outcome = terminate_unix(
            child.id(),
            || child.try_wait().unwrap().is_some(),
            || true,
            Duration::from_millis(500),
        );
        assert!(outcome.confirmed());
        (outcome, child.wait().unwrap())
    }

    #[cfg(unix)]
    #[test]
    fn graceful_terminate_escalates_when_sigterm_ignored() {
        use std::os::unix::process::ExitStatusExt;
        let (outcome, status) = test_stop_child("trap '' TERM");
        assert_eq!(outcome, StopOutcome::Killed);
        assert_eq!(status.signal(), Some(libc::SIGKILL));
    }

    #[cfg(unix)]
    #[test]
    fn graceful_terminate_cooperative_exit() {
        let (outcome, status) = test_stop_child("trap 'exit 23' TERM");
        assert_eq!(outcome, StopOutcome::Terminated);
        assert_eq!(status.code(), Some(23));
    }

    #[test]
    fn geo_file_ok_rejects_truncated_metadb() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("geoip.metadb");
        // ≥4MB 但无尾部标志：模拟弱网截断残片（旧校验会放行，内核必拒）
        std::fs::write(&p, vec![0u8; 5_000_000]).unwrap();
        assert!(!geo_file_ok("geoip.metadb", &p, 4_000_000));
        // 带尾部标志的真文件形态
        let mut good = vec![0u8; 5_000_000];
        good.extend_from_slice(b"\xab\xcd\xefMaxMind.com");
        std::fs::write(&p, good).unwrap();
        assert!(geo_file_ok("geoip.metadb", &p, 4_000_000));
        // 过小直接拒
        std::fs::write(&p, vec![0u8; 1000]).unwrap();
        assert!(!geo_file_ok("geoip.metadb", &p, 4_000_000));
    }

    #[test]
    fn geo_file_ok_non_metadb_size_only() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("geosite.dat");
        std::fs::write(&p, vec![7u8; 600_000]).unwrap();
        assert!(geo_file_ok("geosite.dat", &p, 500_000));
    }

    /// 供其他模块的真内核测试取空闲混合端口（默认 7897 真机上常被占）。
    pub(crate) fn tests_free_port() -> u16 {
        free_port().unwrap()
    }

    #[test]
    fn dead_descriptor_is_removed_without_waiting_for_controller_timeout() {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        let mut child = crate::process::command("sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        #[cfg(windows)]
        let mut child = crate::process::command("cmd.exe")
            .args(["/C", "exit", "0"])
            .spawn()
            .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        let manager = CoreManager::new(PathBuf::from("missing-core"), dir.path().into());
        manager
            .shared
            .write_descriptor(
                pid,
                free_port().unwrap(),
                "test-secret",
                Path::new("missing-core"),
            )
            .unwrap();
        let started = Instant::now();
        assert!(!manager.adopt_external());
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(!Shared::descriptor_path(dir.path()).exists());
    }

    #[cfg(windows)]
    #[test]
    fn windows_liveness_detects_exit_with_child_handle_still_retained() {
        let mut child = crate::process::command("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 5",
            ])
            .spawn()
            .unwrap();
        let pid = child.id();
        assert!(pid_is_alive(pid));
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(!pid_is_alive(pid));
    }

    /// 真内核测试共享网络与端口，必须串行执行（跨模块共享同一把锁）。
    pub(crate) static REAL_CORE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn free_port_returns_usable_port() {
        let p = free_port().unwrap();
        assert!(p > 0);
    }

    #[test]
    fn adopted_cache_requires_a_known_unchanged_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        let mut record: CoreDescriptor =
            serde_json::from_str(r#"{"pid":1,"port":12345,"secret":"test"}"#).unwrap();
        std::fs::write(&path, "mode: rule\n").unwrap();
        assert!(
            record.verified_config(&path).is_none(),
            "legacy records cannot guess live config"
        );
        record.config = Some("mode: rule\n".into());
        assert_eq!(record.verified_config(&path), record.config);
        std::fs::write(&path, "mode: direct\n").unwrap();
        assert!(
            record.verified_config(&path).is_none(),
            "uncommitted disk edits are not live config"
        );
        std::fs::remove_file(&path).unwrap();
        assert!(record.verified_config(&path).is_none());
    }

    #[test]
    fn preflight_does_not_bypass_a_live_listener() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        assert_eq!(
            check_mixed_port_available(listener.local_addr().unwrap())
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::AddrInUse
        );
    }

    #[test]
    fn invalid_configuration_does_not_leave_starting_state() {
        let dir = tempfile::tempdir().unwrap();
        let manager = CoreManager::new(std::env::current_exe().unwrap(), dir.path().to_path_buf());
        assert!(manager
            .start(
                "proxies: [",
                &RuntimeConfig {
                    mixed_port: free_port().unwrap(),
                    ..RuntimeConfig::default()
                }
            )
            .is_err());
        assert!(matches!(manager.status(), CoreStatus::Crashed(_)));
        assert!(manager.pid().is_none());
    }

    fn wait_test_core(manager: &CoreManager) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while manager.status() != CoreStatus::Running {
            assert!(
                Instant::now() < deadline,
                "core not ready: {:?}",
                manager.status()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Local-only regression for the actual macOS root -> user TIME_WAIT handover.
    /// Uses isolated ports/configs, TUN disabled, and never changes system proxy.
    #[cfg(target_os = "macos")]
    #[test]
    fn real_privileged_to_user_port_handover() {
        use std::io::Read;
        let _serial = REAL_CORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (Ok(root_bin), Ok(user_bin)) = (
            std::env::var("CROSSBOW_ROOT_CORE_BIN"),
            std::env::var("CROSSBOW_CORE_BIN"),
        ) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let root = CoreManager::new(root_bin.into(), dir.path().to_path_buf());
        root.set_options(CoreOptions {
            ensure_geo_files: false,
            ..CoreOptions::default()
        });
        let rt = RuntimeConfig {
            mixed_port: free_port().unwrap(),
            ..RuntimeConfig::default()
        };
        let base = "proxies: []\nrules: ['MATCH,DIRECT']\n";
        root.start(base, &rt).unwrap();
        wait_test_core(&root);
        let mut client = TcpStream::connect(("127.0.0.1", rt.mixed_port)).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        client.write_all(b"GET http://127.0.0.1:1/ HTTP/1.1\r\nHost: 127.0.0.1:1\r\nConnection: close\r\n\r\n").unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1"));
        drop(client);
        assert!(root.stop());
        let address = SocketAddr::from(([127, 0, 0, 1], rt.mixed_port));
        assert!(!port_has_listener(address));
        assert_eq!(
            check_mixed_port_available(address).unwrap_err().kind(),
            std::io::ErrorKind::AddrInUse
        );
        let user = CoreManager::new(user_bin.into(), dir.path().to_path_buf());
        user.set_options(CoreOptions {
            ensure_geo_files: false,
            ..CoreOptions::default()
        });
        let began = Instant::now();
        user.start(base, &rt).unwrap();
        wait_test_core(&user);
        assert!(began.elapsed() < Duration::from_secs(90));
        assert!(user.stop());
    }

    #[test]
    fn real_handoff_keeps_reload_snapshot_and_process() {
        let _serial = REAL_CORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(bin) = std::env::var("CROSSBOW_CORE_BIN") else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let previous = CoreManager::new(bin.clone().into(), dir.path().to_path_buf());
        previous.set_options(CoreOptions {
            ensure_geo_files: false,
            ..CoreOptions::default()
        });
        previous
            .start(
                "proxies: []\nrules: ['MATCH,DIRECT']\n",
                &RuntimeConfig {
                    mixed_port: free_port().unwrap(),
                    ..RuntimeConfig::default()
                },
            )
            .unwrap();
        wait_test_core(&previous);
        let pid = previous.pid();
        let original = previous.current_config();
        previous.prepare_handoff().unwrap();
        // Simulate the old GUI exiting, without leaving its watcher in this test process.
        previous.shared.inner.lock().unwrap().stopping = true;
        let mut child = previous.shared.inner.lock().unwrap().child.take().unwrap();
        let reaper = std::thread::spawn(move || child.wait());
        let replacement = CoreManager::new(bin.into(), dir.path().to_path_buf());
        assert!(replacement.adopt_external());
        assert_eq!(replacement.current_config(), original);
        let config = original.unwrap().replace("mode: rule", "mode: direct");
        replacement.write_config_file(&config).unwrap();
        let (port, secret) = replacement.controller().unwrap();
        crate::engine::api::Controller { port, secret }
            .reload_config(&replacement.config_path())
            .unwrap();
        replacement.replace_cached_config(config.clone());
        assert_eq!(replacement.pid(), pid);
        assert_eq!(
            Shared::read_descriptor(dir.path()).unwrap().config,
            Some(config)
        );
        assert!(replacement.stop());
        reaper.join().unwrap().unwrap();
    }

    #[test]
    fn start_rejects_missing_binary() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = CoreManager::new(dir.path().join("no-such-bin"), dir.path().to_path_buf());
        let err = mgr
            .start("proxies: []", &RuntimeConfig::default())
            .unwrap_err();
        assert!(err.contains("not found"));
        assert!(matches!(mgr.status(), CoreStatus::Crashed(_)));
    }

    /// 假内核：进程不提供控制器 → Starting 超时 → Crashed。
    /// 二进制不存在时 spawn 失败，同样收敛到 Crashed（覆盖 Windows CI）。
    #[test]
    fn readiness_timeout_marks_crashed() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = CoreManager::new(PathBuf::from("/bin/sleep"), dir.path().to_path_buf());
        mgr.set_options(CoreOptions {
            readiness_timeout_secs: 1,
            ensure_geo_files: false,
            ..CoreOptions::default()
        });
        // 默认 7897 在真机上常被其他代理占用（预检会直接拒绝），测试用空闲端口
        let _ = mgr.start(
            "",
            &RuntimeConfig {
                mixed_port: free_port().unwrap(),
                ..RuntimeConfig::default()
            },
        );
        for _ in 0..80 {
            if matches!(mgr.status(), CoreStatus::Crashed(_)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("expected Crashed, got {:?}", mgr.status());
    }

    fn record_test_external_start(manager: &CoreManager, pid: u32, port: u16, secret: &str) {
        {
            let mut inner = manager.shared.inner.lock().unwrap();
            inner.status = CoreStatus::Starting;
            inner.stopping = false;
            inner.external_pid = Some(pid);
            inner.external_binary = Some(manager.shared.binary_path.clone());
            inner.detached = true;
            inner.controller = Some((port, secret.into()));
        }
        manager
            .shared
            .write_descriptor(pid, port, secret, &manager.shared.binary_path)
            .unwrap();
    }

    /// 替身租约用已退出的进程，测试不会给任何活进程发信号。
    fn exited_test_pid() -> u32 {
        #[cfg(unix)]
        let mut child = crate::process::command("sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        #[cfg(windows)]
        let mut child = crate::process::command("cmd.exe")
            .args(["/C", "exit", "0"])
            .spawn()
            .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        pid
    }

    #[test]
    fn stale_service_probe_cannot_stop_or_publish_over_a_new_start() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let dir = tempfile::tempdir().unwrap();
        let manager = CoreManager::new(std::env::current_exe().unwrap(), dir.path().into());
        let pid = std::process::id();
        record_test_external_start(&manager, pid, 18080, "new-start");
        let descriptor = std::fs::read(Shared::descriptor_path(dir.path())).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let service_calls = calls.clone();
        manager.set_service_stop_hook(move || {
            service_calls.fetch_add(1, Ordering::SeqCst);
            Err("must not stop a newer start".into())
        });
        let notices = Arc::new(AtomicUsize::new(0));
        let callback_notices = notices.clone();
        manager.set_callback(Arc::new(move |_| {
            callback_notices.fetch_add(1, Ordering::SeqCst);
        }));

        // 分别覆盖 PID 改变、PID 复用，以及 PID/端口同时复用。
        for (old_pid, old_port, old_secret) in [
            (pid + 1, 18080, "new-start"),
            (pid, 18081, "new-start"),
            (pid, 18080, "old-start"),
        ] {
            for status in [CoreStatus::Starting, CoreStatus::Running] {
                manager.shared.inner.lock().unwrap().status = status.clone();
                let old_probe = ControllerProbe {
                    port: old_port,
                    secret: old_secret.into(),
                };
                assert!(!manager
                    .shared
                    .external_start_is_current(old_pid, &old_probe));
                for ready in [false, true] {
                    finish_external_probe(&manager.shared, old_pid, &old_probe, ready);
                    assert_eq!(manager.status(), status);
                    assert_eq!(manager.pid(), Some(pid));
                    assert_eq!(
                        std::fs::read(Shared::descriptor_path(dir.path())).unwrap(),
                        descriptor
                    );
                }
            }
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(notices.load(Ordering::SeqCst), 0);

        // 当前批次成功仍然正常上报，而旧批次连等待窗口也不再消耗。
        let current = ControllerProbe {
            port: 18080,
            secret: "new-start".into(),
        };
        finish_external_probe(&manager.shared, pid, &current, true);
        assert_eq!(manager.status(), CoreStatus::Running);
        assert_eq!(notices.load(Ordering::SeqCst), 1);
        manager.set_options(CoreOptions {
            readiness_timeout_secs: 30,
            ..CoreOptions::default()
        });
        let shared = manager.shared.clone();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let obsolete = std::thread::spawn(move || {
            probe_external_core(shared, pid, 18080, "old-start".into());
            done_tx.send(()).unwrap();
        });
        done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("obsolete probe must cancel promptly");
        obsolete.join().unwrap();
        assert_eq!(manager.status(), CoreStatus::Running);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(notices.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn service_probe_timeout_preserves_lease_when_service_stop_fails() {
        let dir = tempfile::tempdir().unwrap();
        let manager = CoreManager::new(std::env::current_exe().unwrap(), dir.path().into());
        let pid = std::process::id();
        record_test_external_start(&manager, pid, 18080, "current-start");
        let descriptor = std::fs::read(Shared::descriptor_path(dir.path())).unwrap();
        manager.set_service_stop_hook(|| Err("service stop failed".into()));
        finish_external_probe(
            &manager.shared,
            pid,
            &ControllerProbe {
                port: 18080,
                secret: "current-start".into(),
            },
            false,
        );
        assert!(matches!(manager.status(), CoreStatus::Crashed(_)));
        assert_eq!(manager.pid(), Some(pid));
        assert_eq!(
            manager.shared.inner.lock().unwrap().external_binary,
            Some(manager.shared.binary_path.clone())
        );
        assert_eq!(
            std::fs::read(Shared::descriptor_path(dir.path())).unwrap(),
            descriptor
        );
    }

    #[test]
    fn service_probe_cleanup_finishes_before_stop_and_replacement_start() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::mpsc;

        let dir = tempfile::tempdir().unwrap();
        let manager = CoreManager::new(std::env::current_exe().unwrap(), dir.path().into());
        let pid = exited_test_pid();
        record_test_external_start(&manager, pid, 18080, "old-start");
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let calls = AtomicUsize::new(0);
        manager.set_service_stop_hook(move || {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                entered_tx.send(()).unwrap();
                release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
            }
            Ok(())
        });
        let shared = manager.shared.clone();
        let cleanup = std::thread::spawn(move || {
            finish_external_probe(
                &shared,
                pid,
                &ControllerProbe {
                    port: 18080,
                    secret: "old-start".into(),
                },
                false,
            );
        });
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        let replacement = manager.clone();
        let (attempted_tx, attempted_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let restart = std::thread::spawn(move || {
            attempted_tx.send(()).unwrap();
            assert!(replacement.stop());
            let result = replacement.start_with_spawner(
                "proxies: []\nrules: ['MATCH,DIRECT']\n",
                &RuntimeConfig {
                    mixed_port: free_port().unwrap(),
                    ..RuntimeConfig::default()
                },
                Box::new(move |_, _| Ok(pid)),
            );
            done_tx.send(()).unwrap();
            result
        });
        attempted_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let waited_for_cleanup = matches!(
            done_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        // 先放行再断言，回归失败时也不会把测试线程挂在服务替身里。
        release_tx.send(()).unwrap();
        cleanup.join().unwrap();
        restart.join().unwrap().unwrap();
        assert!(
            waited_for_cleanup,
            "stop/start bypassed in-flight service cleanup"
        );
        assert_eq!(manager.status(), CoreStatus::Starting);
        assert_eq!(manager.pid(), Some(pid));
        let descriptor =
            Shared::read_descriptor(dir.path()).expect("new descriptor must survive old cleanup");
        assert_eq!(descriptor.pid, pid);
        assert_ne!(descriptor.secret, "old-start");
        assert_eq!(
            manager.controller(),
            Some((descriptor.port, descriptor.secret))
        );
        assert!(manager.stop());
    }

    /// #343 的标准回归：服务托管租约没有二进制身份时，再次启动必须先请服务停。
    /// 服务停失败时不能对活着的 PID 发信号；服务确认停掉后，不能再卡在同一句错误上。
    #[cfg(windows)]
    #[test]
    fn crashed_service_lease_without_binary_is_released_only_through_the_service() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("mihomo.exe");
        std::fs::write(&bin, b"not-a-real-core").unwrap();
        let mgr = CoreManager::new(bin, dir.path().to_path_buf());
        let mut sleeper = crate::process::command("powershell")
            .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn sleeper");
        let pid = sleeper.id();
        assert!(pid_is_alive(pid), "sleeper exited before the restart check");
        {
            let mut inner = mgr.shared.inner.lock().unwrap();
            inner.status = CoreStatus::Crashed("服务托管内核启动失败/超时".into());
            inner.external_pid = Some(pid);
            inner.external_binary = None;
            inner.detached = true;
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_hook = calls.clone();
        mgr.set_service_stop_hook(move || {
            calls_for_hook.fetch_add(1, Ordering::SeqCst);
            Err("no core".into())
        });
        let err = mgr
            .start(
                "proxies: []\nrules:\n  - MATCH,DIRECT\n",
                &RuntimeConfig {
                    mixed_port: free_port().unwrap(),
                    ..RuntimeConfig::default()
                },
            )
            .unwrap_err();
        assert!(
            err.contains(crate::engine::restart::RESTART_BLOCKED),
            "{err}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(
            pid_is_alive(pid),
            "refused signal must leave the unverified process alive"
        );
        assert_eq!(mgr.shared.inner.lock().unwrap().external_pid, Some(pid));

        let calls_for_release = calls.clone();
        mgr.set_service_stop_hook(move || {
            calls_for_release.fetch_add(1, Ordering::SeqCst);
            let _ = crate::process::command("taskkill")
                .args(["/F", "/PID", &pid.to_string()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            for _ in 0..50 {
                if !pid_is_alive(pid) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(())
        });
        match mgr.start(
            "proxies: []\nrules:\n  - MATCH,DIRECT\n",
            &RuntimeConfig {
                mixed_port: free_port().unwrap(),
                ..RuntimeConfig::default()
            },
        ) {
            Ok(()) => {}
            Err(err) => assert!(
                !err.contains(crate::engine::restart::RESTART_BLOCKED),
                "{err}"
            ),
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(mgr.shared.inner.lock().unwrap().external_pid.is_none());
        let _ = sleeper.kill();
        let _ = sleeper.wait();
    }

    #[cfg(windows)]
    #[test]
    fn service_spawn_records_the_binary_the_service_actually_launches() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("engine-mihomo.exe");
        std::fs::write(&bin, b"not-a-real-core").unwrap();
        let mgr = CoreManager::new(bin.clone(), dir.path().to_path_buf());
        mgr.set_options(CoreOptions {
            readiness_timeout_secs: 30,
            ensure_geo_files: false,
            ..CoreOptions::default()
        });
        let mut sleeper = crate::process::command("powershell")
            .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn sleeper");
        let pid = sleeper.id();
        let result = mgr.start_with_spawner(
            "proxies: []\nrules:\n  - MATCH,DIRECT\n",
            &RuntimeConfig {
                mixed_port: free_port().unwrap(),
                ..RuntimeConfig::default()
            },
            Box::new(move |_, _| Ok(pid)),
        );
        let recorded = result
            .as_ref()
            .ok()
            .and_then(|_| mgr.shared.inner.lock().unwrap().external_binary.clone());
        mgr.set_service_stop_hook(move || {
            let _ = crate::process::command("taskkill")
                .args(["/F", "/PID", &pid.to_string()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            Ok(())
        });
        let _ = mgr.stop();
        let _ = sleeper.kill();
        let _ = sleeper.wait();
        let expected = crate::service_client::pinned_core_path().unwrap_or(bin);
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(recorded.unwrap(), expected);
    }

    /// Network-free integration: real mihomo, private ports and temporary files.
    #[test]
    fn real_core_reload_preserves_pid_and_restores_rejected_config() {
        let _serial = REAL_CORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(bin) = std::env::var("CROSSBOW_CORE_BIN") else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let mgr = CoreManager::new(PathBuf::from(bin), dir.path().to_path_buf());
        mgr.set_options(CoreOptions {
            ensure_geo_files: false,
            ..CoreOptions::default()
        });
        let rt = RuntimeConfig {
            mixed_port: free_port().unwrap(),
            ..RuntimeConfig::default()
        };
        let base = "proxies: []\nrules: ['MATCH,DIRECT']\n";
        mgr.start(base, &rt).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while mgr.status() != CoreStatus::Running {
            assert!(
                Instant::now() < deadline,
                "core not ready: {:?}",
                mgr.status()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        let pid = mgr.pid();
        let (port, secret) = mgr.controller().unwrap();
        let controller = crate::engine::api::Controller {
            port,
            secret: secret.clone(),
        };
        let old = mgr.current_config().unwrap();
        let new_config = old.replace("mode: rule", "mode: direct");
        assert_ne!(old, new_config);
        mgr.write_config_file(&new_config).unwrap();
        controller.reload_config(&mgr.config_path()).unwrap();
        mgr.replace_cached_config(new_config.clone());
        assert_eq!(mgr.pid(), pid);
        assert_eq!(mgr.controller(), Some((port, secret)));
        assert_eq!(mgr.current_config(), Some(new_config.clone()));
        mgr.write_config_file("proxies: [invalid-proxy-entry]")
            .unwrap();
        assert!(controller.reload_config(&mgr.config_path()).is_err());
        assert_eq!(mgr.current_config(), Some(new_config));
        mgr.write_config_file(&old).unwrap();
        controller.reload_config(&mgr.config_path()).unwrap();
        mgr.replace_cached_config(old.clone());
        assert_eq!(mgr.pid(), pid);
        assert_eq!(mgr.current_config(), Some(old));
        assert!(mgr.stop());
    }

    /// 真实内核冒烟：CROSSBOW_CORE_BIN 指向 mihomo 时才运行。
    /// 验证 启动 → Running → 混合端口可代理 → 停止 → Stopped。
    #[test]
    fn real_core_lifecycle_and_proxy_roundtrip() {
        let _serial = REAL_CORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(bin) = std::env::var("CROSSBOW_CORE_BIN") else {
            eprintln!("skip: CROSSBOW_CORE_BIN not set");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let mgr = CoreManager::new(PathBuf::from(bin), dir.path().to_path_buf());
        mgr.set_options(CoreOptions {
            ensure_geo_files: false,
            ..CoreOptions::default()
        });
        let port = free_port().unwrap();
        let base = "proxies: []\nrules:\n  - MATCH,DIRECT\n".to_string();
        mgr.start(
            &base,
            &RuntimeConfig {
                mixed_port: port,
                ..RuntimeConfig::default()
            },
        )
        .unwrap();
        let mut running = false;
        for _ in 0..100 {
            if mgr.status() == CoreStatus::Running {
                running = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(running, "core never reached Running: {:?}", mgr.status());

        // 通过混合端口代理访问 generate_204，期望 204
        let out = Command::new("curl")
            .args([
                "-s",
                "-o",
                "/dev/null",
                "-w",
                "%{http_code}",
                "--max-time",
                "15",
                "-x",
                &format!("http://127.0.0.1:{port}"),
                "http://www.gstatic.com/generate_204",
            ])
            .output()
            .expect("curl");
        let code = String::from_utf8_lossy(&out.stdout).to_string();
        assert_eq!(code, "204", "proxy roundtrip failed: {code}");

        assert!(mgr.stop());
        assert_eq!(mgr.status(), CoreStatus::Stopped);
        #[cfg(unix)]
        {
            let decision = std::fs::read_to_string(dir.path().join("core-decision.log")).unwrap();
            assert!(decision.contains("outcome=Terminated"));
            let stdio = std::fs::read_to_string(dir.path().join("core-stdio.log")).unwrap();
            assert!(stdio.contains("Mihomo shutting down"));
        }
    }

    /// 回归：崩溃自动重启必须用本启动链缓存的内容重写 config.yaml。
    /// 旧实现重启时原样复用磁盘文件——换引擎交错时会把新引擎的配置
    /// 喂给旧内核（mihomo 拿到 sing-box JSON，控制器永不就绪）。
    #[test]
    fn real_core_crash_restart_rewrites_config() {
        let _serial = REAL_CORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(bin) = std::env::var("CROSSBOW_CORE_BIN") else {
            eprintln!("skip: CROSSBOW_CORE_BIN not set");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let mgr = CoreManager::new(PathBuf::from(bin), dir.path().to_path_buf());
        mgr.set_options(CoreOptions {
            ensure_geo_files: false,
            ..CoreOptions::default()
        });
        let port = free_port().unwrap();
        let base = "proxies: []\nrules:\n  - MATCH,DIRECT\n".to_string();
        mgr.start(
            &base,
            &RuntimeConfig {
                mixed_port: port,
                ..RuntimeConfig::default()
            },
        )
        .unwrap();
        let mut running = false;
        for _ in 0..100 {
            if mgr.status() == CoreStatus::Running {
                running = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(running, "core never reached Running: {:?}", mgr.status());

        // 杀内核触发崩溃重启，再把磁盘配置篡改成垃圾——重启必须重写
        {
            let mut inner = mgr.shared.inner.lock().unwrap();
            if let Some(c) = inner.child.as_mut() {
                let _ = c.kill();
            }
        }
        let cfg_path = dir.path().join("config.yaml");
        std::fs::write(&cfg_path, "# TAMPERED\n").unwrap();

        let mut ok = false;
        for _ in 0..200 {
            let rewritten = std::fs::read_to_string(&cfg_path)
                .map(|c| c.contains("mixed-port") && !c.contains("TAMPERED"))
                .unwrap_or(false);
            if mgr.status() == CoreStatus::Running && rewritten {
                ok = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(
            ok,
            "restart should rewrite config and reach Running: {:?}",
            mgr.status()
        );
        mgr.stop();
        assert_eq!(mgr.status(), CoreStatus::Stopped);
    }

    /// 收养场景：无 Child 只有 external_pid，stop() 应能收走外部内核。
    #[cfg(unix)] // Uses /bin/sleep and the Unix external-pid lifecycle.
    #[test]
    fn stop_kills_adopted_external_core() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = CoreManager::new(PathBuf::from("/bin/sleep"), dir.path().to_path_buf());
        let mut sleeper = Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleeper");
        {
            let mut inner = mgr.shared.inner.lock().unwrap();
            inner.detached = true;
            inner.status = CoreStatus::Running;
            inner.controller = Some((1, "x".into()));
            inner.external_pid = Some(sleeper.id());
            inner.external_binary = Some(PathBuf::from("/bin/sleep"));
        }
        let pid = sleeper.id();
        assert_eq!(mgr.pid(), Some(pid));
        // A real adopted core is reaped by its original parent/launchd. Reap
        // concurrently so an exited zombie is not mistaken for a live core.
        let reaper = std::thread::spawn(move || sleeper.wait());
        assert!(mgr.stop());
        let _ = reaper.join().unwrap();
        assert!(!kill_pid_probe(pid), "external core should be dead");
        assert!(!Shared::descriptor_path(dir.path()).exists());
    }

    /// 交接语义：detached 后管理器 Drop 不得杀内核，且描述符已写。
    #[cfg(unix)] // Uses /bin/sleep and kill(1) to probe detached children.
    #[test]
    fn detach_survives_manager_drop() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = CoreManager::new(PathBuf::from("/bin/sleep"), dir.path().to_path_buf());
        let sleeper = Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleeper");
        let child_pid = sleeper.id();
        {
            let mut inner = mgr.shared.inner.lock().unwrap();
            inner.detached = true;
            inner.status = CoreStatus::Running;
            inner.controller = Some((19099, "sec".into()));
            inner.child = Some(sleeper); // 所有权交给管理器
        }
        mgr.detach();
        drop(mgr); // 触发 Shared::Drop
        std::thread::sleep(Duration::from_millis(300));
        let alive = Command::new("kill")
            .arg("-0")
            .arg(child_pid.to_string())
            .output();
        assert!(
            alive.unwrap().status.success(),
            "detached core must survive drop"
        );
        assert!(
            Shared::descriptor_path(dir.path()).exists(),
            "descriptor written"
        );
        let _ = Command::new("kill")
            .arg("-9")
            .arg(child_pid.to_string())
            .output();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn privileged_scripts_compile_without_executing_authorization() {
        let dir = tempfile::tempdir().unwrap();
        for (index, command) in ["kill -9 123", "printf '\"quoted\" \\ path'"]
            .iter()
            .enumerate()
        {
            let script = privileged_shell_script(command);
            let output = Command::new("/usr/bin/osacompile")
                .arg("-e")
                .arg(&script)
                .arg("-o")
                .arg(dir.path().join(format!("{index}.scpt")))
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn update_handoff_adopts_and_preserves_actual_binary_across_generations() {
        use std::io::Read;
        let dir = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut buffer = [0; 2048];
            let _ = stream.read(&mut buffer).unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .unwrap();
        });
        let previous = CoreManager::new(PathBuf::from("/bin/sleep"), dir.path().to_path_buf());
        let snapshot = "mode: rule\n".to_string();
        std::fs::write(previous.config_path(), &snapshot).unwrap();
        *previous.shared.config_cache.lock().unwrap() = Some(snapshot.clone());
        let sleeper = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let pid = sleeper.id();
        {
            let mut inner = previous.shared.inner.lock().unwrap();
            inner.child = Some(sleeper);
            inner.status = CoreStatus::Running;
            inner.controller = Some((port, "test-secret".into()));
        }
        assert_eq!(previous.prepare_handoff().unwrap(), Some(pid));
        let replacement = CoreManager::new(PathBuf::from("/bin/false"), dir.path().to_path_buf());
        assert!(replacement.adopt_external());
        server.join().unwrap();
        assert_eq!(replacement.current_config(), Some(snapshot));
        let reloaded = "mode: direct\n".to_string();
        replacement.write_config_file(&reloaded).unwrap();
        replacement.replace_cached_config(reloaded.clone());
        assert_eq!(
            Shared::read_descriptor(dir.path()).unwrap().config,
            Some(reloaded)
        );
        assert_eq!(replacement.binary_path(), PathBuf::from("/bin/sleep"));
        assert_eq!(replacement.prepare_handoff().unwrap(), Some(pid));
        let record = Shared::read_descriptor(dir.path()).unwrap();
        assert_eq!(record.binary_path, Some(PathBuf::from("/bin/sleep")));
        assert_eq!(record.pid, pid);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(Shared::descriptor_path(dir.path()))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let mut child = previous.shared.inner.lock().unwrap().child.take().unwrap();
        drop(previous);
        let reaper = std::thread::spawn(move || child.wait());
        assert!(replacement.stop());
        let _ = reaper.join().unwrap();
        assert!(!Shared::descriptor_path(dir.path()).exists());
    }

    /// 服务租约没有进程身份时，退出恢复不得猜测身份或探测控制器。
    #[test]
    fn failed_exit_resume_preserves_unverified_service_lease() {
        let dir = tempfile::tempdir().unwrap();
        let manager = CoreManager::new(std::env::current_exe().unwrap(), dir.path().into());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        {
            let mut inner = manager.shared.inner.lock().unwrap();
            inner.external_pid = Some(std::process::id());
            inner.external_binary = None;
            inner.stopping = true;
        }
        {
            let _lifecycle = manager.shared.lifecycle.lock().unwrap();
            manager.resume_after_failed_stop(Some((
                listener.local_addr().unwrap().port(),
                "secret".into(),
            )));
        }
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
        let mut inner = manager.shared.inner.lock().unwrap();
        assert_eq!(inner.external_pid, Some(std::process::id()));
        assert!(inner.external_binary.is_none());
        assert!(inner.controller.is_none());
        assert!(inner.stopping);
        inner.external_pid = None;
    }

    /// 停止授权取消后恢复控制连接，且恢复过程不再次发送停止请求。
    #[cfg(unix)]
    #[test]
    fn failed_exit_stop_resumes_authenticated_core_without_another_kill() {
        use std::io::Read;
        let dir = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut buffer = [0; 2048];
            let size = stream.read(&mut buffer).unwrap();
            assert!(String::from_utf8_lossy(&buffer[..size])
                .to_lowercase()
                .contains("authorization: bearer test-secret"));
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .unwrap();
        });
        let manager = CoreManager::new(PathBuf::from("/bin/sleep"), dir.path().to_path_buf());
        let mut child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        let reaper = std::thread::spawn(move || child.wait());
        // 模拟停止授权被取消后留下的状态，恢复只能探测，不能再次杀进程。
        {
            let mut inner = manager.shared.inner.lock().unwrap();
            inner.external_pid = Some(pid);
            inner.external_binary = Some(PathBuf::from("/bin/sleep"));
            inner.stopping = true;
            inner.status = CoreStatus::Crashed("cancelled".into());
        }
        {
            let _lifecycle = manager.shared.lifecycle.lock().unwrap();
            manager.resume_after_failed_stop(Some((port, "test-secret".into())));
        }
        server.join().unwrap();
        assert_eq!(manager.controller(), Some((port, "test-secret".into())));
        assert_eq!(manager.status(), CoreStatus::Running);
        assert!(!manager.shared.inner.lock().unwrap().stopping);
        assert!(pid_is_alive(pid));
        assert!(manager.stop());
        reaper.join().unwrap().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn failed_handoff_keeps_core_owned() {
        let dir = tempfile::tempdir().unwrap();
        let manager = CoreManager::new(PathBuf::from("/bin/sleep"), dir.path().to_path_buf());
        let sleeper = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        {
            let mut inner = manager.shared.inner.lock().unwrap();
            inner.child = Some(sleeper);
            inner.status = CoreStatus::Running;
            inner.controller = Some((19099, "test-secret".into()));
        }
        std::fs::create_dir(Shared::descriptor_path(dir.path())).unwrap();
        assert!(manager.prepare_handoff().is_err());
        assert!(!manager.shared.inner.lock().unwrap().detached);
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "failed write must remove temporary files"
        );
        assert!(manager.stop());
    }

    #[cfg(unix)]
    #[test]
    fn unknown_live_identity_is_not_reported_as_cleaned() {
        let dir = tempfile::tempdir().unwrap();
        let manager = CoreManager::new(PathBuf::from("/bin/sleep"), dir.path().to_path_buf());
        let mut sleeper = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let missing = dir.path().join("unavailable-identity");
        manager
            .shared
            .write_descriptor(sleeper.id(), 19099, "test-secret", &missing)
            .unwrap();
        {
            let mut inner = manager.shared.inner.lock().unwrap();
            inner.external_pid = Some(sleeper.id());
            inner.external_binary = Some(missing);
            inner.status = CoreStatus::Running;
        }
        assert!(!manager.stop());
        assert!(Shared::descriptor_path(dir.path()).exists());
        assert!(
            kill_pid_probe(sleeper.id()),
            "identity uncertainty must not send a signal"
        );
        sleeper.kill().unwrap();
        sleeper.wait().unwrap();
        drop(manager);
    }

    #[test]
    fn updater_recovery_accepts_only_our_exact_temporary_bundle_mapping() {
        let root = Path::new("/private/tmp/test-root");
        let expected = Path::new("/Applications/Crossbow.app/Contents/MacOS/mihomo");
        assert!(updater_text_path(
            &root.join("tauri_current_appRu7F6K/current_app/Contents/MacOS/mihomo"),
            expected,
            root
        ));
        for bad in [
            "other/current_app/Contents/MacOS/mihomo",
            "tauri_current_app/current_app/Contents/MacOS/mihomo",
            "tauri_current_appABC/other/Contents/MacOS/mihomo",
            "tauri_current_appABC/current_app/Contents/MacOS/sleep",
            "tauri_current_appABC/current_app/Contents/MacOS/mihomo/extra",
            "../tauri_current_appABC/current_app/Contents/MacOS/mihomo",
        ] {
            assert!(!updater_text_path(&root.join(bad), expected, root), "{bad}");
        }
        assert!(!updater_text_path(
            Path::new("/other/tauri_current_appABC/current_app/Contents/MacOS/mihomo"),
            expected,
            root
        ));
        assert!(!updater_text_path(
            &root.join("tauri_current_appABC/current_app/Contents/MacOS/mihomo"),
            Path::new("/other/mihomo"),
            root
        ));
    }

    #[test]
    fn cold_restart_does_not_invent_a_running_core() {
        let dir = tempfile::tempdir().unwrap();
        let manager = CoreManager::new(dir.path().join("missing"), dir.path().to_path_buf());
        assert_eq!(manager.prepare_handoff().unwrap(), None);
        assert!(!Shared::descriptor_path(dir.path()).exists());
    }

    /// 真实内核崩溃自动恢复：Running 中强杀子进程 → 应自动回到 Running。
    #[test]
    fn real_core_crash_auto_recovers() {
        let Ok(bin) = std::env::var("CROSSBOW_CORE_BIN") else {
            eprintln!("skip: CROSSBOW_CORE_BIN not set");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let mgr = CoreManager::new(PathBuf::from(bin), dir.path().to_path_buf());
        mgr.set_options(CoreOptions {
            readiness_timeout_secs: 10,
            ensure_geo_files: false,
            ..CoreOptions::default()
        });
        mgr.start(
            "proxies: []\nrules:\n  - MATCH,DIRECT\n",
            &RuntimeConfig {
                mixed_port: free_port().unwrap(),
                ..RuntimeConfig::default()
            },
        )
        .unwrap();
        let wait_status = |mgr: &CoreManager, want: &dyn Fn(&CoreStatus) -> bool| {
            for _ in 0..150 {
                if want(&mgr.status()) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            false
        };
        assert!(wait_status(&mgr, &|s| *s == CoreStatus::Running));

        // 模拟崩溃：SIGKILL 管理器自己的子进程（不能按名字找，会误伤其他实例）
        let pid = mgr.pid().expect("core child pid");
        Command::new("kill")
            .args(["-9", &pid.to_string()])
            .output()
            .unwrap();

        // 应先观察到 Crashed，随后自动重启回到 Running
        assert!(
            wait_status(&mgr, &|s| matches!(s, CoreStatus::Crashed(_))),
            "no Crashed observed"
        );
        assert!(
            wait_status(&mgr, &|s| *s == CoreStatus::Running),
            "did not recover to Running: {:?}",
            mgr.status()
        );
        mgr.stop();
        assert_eq!(mgr.status(), CoreStatus::Stopped);
    }
}
