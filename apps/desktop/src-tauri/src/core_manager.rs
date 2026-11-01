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
    /// 本次启动链的最终配置内容；spawn（含崩溃重启）每次都据此重写
    /// config.yaml，避免换引擎后旧管理器复用新引擎的配置文件。
    config_cache: Mutex<Option<String>>,
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
        if let Some(cb) = self.callback.lock().unwrap().clone() {
            cb(&status);
        }
    }

    fn spawn_child(&self) -> Result<(), String> {
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
        if crate::core_download::is_core_bin(&self.binary_path, "sing-box") {
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
        {
            let mut inner = self.inner.lock().unwrap();
            // 自有子进程接管：收养角色终结，外部 pid 监视线程据此静默退场
            inner.external_pid = None;
            inner.detached = false;
            inner.child = Some(child);
        }
        if let Some((port, secret)) = self.inner.lock().unwrap().controller.clone() {
            if let Err(error) = self.write_descriptor(pid, port, &secret, &self.binary_path) {
                eprintln!("could not persist core descriptor: {error}");
            }
        }
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
        Ok(())
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
            let _ = child.kill();
            // setuid 内核以 root 运行：普通用户 SIGKILL 报 EPERM，child.wait() 会
            // 永久阻塞。轮询确认退出；超时仍存活则管理员授权补杀（macOS），
            // 用户拒绝授权时放弃句柄（进程交给 launchd，绝不阻塞停止流程）。
            let pid = child.id();
            let mut gone = false;
            for _ in 0..40 {
                match child.try_wait() {
                    Ok(Some(_)) => {
                        gone = true;
                        break;
                    }
                    Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                    Err(_) => break,
                }
            }
            if !gone {
                #[cfg(target_os = "macos")]
                if !ROOT_KILL_PROMPT_DISABLED.load(std::sync::atomic::Ordering::SeqCst) {
                    escalate_admin_kill(pid);
                }
                for _ in 0..40 {
                    match child.try_wait() {
                        Ok(Some(_)) => {
                            gone = true;
                            break;
                        }
                        Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                        Err(_) => break,
                    }
                }
                if !gone {
                    eprintln!("core kill: pid {pid} survived SIGKILL (root?); giving up handle");
                }
            }
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
            let stopped = if crate::linux_service::managed_pid() == Some(pid) {
                match crate::linux_service::stop_core() {
                    Ok(stopped) => stopped,
                    Err(error) => {
                        eprintln!("Linux service cleanup failed: {error}");
                        false
                    }
                }
            } else {
                kill_external(pid, binary.as_deref())
            };
            #[cfg(not(target_os = "linux"))]
            let stopped = kill_external(pid, binary.as_deref());
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
                config_cache: Mutex::new(None),
                inner: Mutex::new(Inner {
                    child: None,
                    status: CoreStatus::Stopped,
                    stopping: false,
                    restarts: VecDeque::new(),
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
        *self.shared
            .config_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(content);
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
    pub fn start_with_spawner(
        &self,
        base_config: &str,
        engine_rt: &RuntimeConfig,
        spawner: Box<dyn FnOnce(&Path, &Path) -> Result<u32, String> + Send>,
    ) -> Result<(), String> {
        self.start_internal(base_config, engine_rt, Some(spawner))
    }

    fn start_internal(
        &self,
        base_config: &str,
        engine_rt: &RuntimeConfig,
        spawner: Option<Box<dyn FnOnce(&Path, &Path) -> Result<u32, String> + Send>>,
    ) -> Result<(), String> {
        // 上一次尝试还在探测中（Starting）：先收掉再启动，避免新配置写入后
        // 被旧探测超时路径误杀/误报。
        if self.status() == CoreStatus::Starting {
            if !self.stop() {
                return Err("无法确认上一次启动的内核已退出，已中止新启动".into());
            }
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
        let has_tracked_process = {
            let inner = self.shared.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.child.is_some() || inner.external_pid.is_some()
        };
        if has_tracked_process && !self.shared.kill_child() {
            let msg = "无法确认上一次内核已退出，已中止新启动".to_string();
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
            let stopped_via_service = crate::service_client::stop_core().is_ok();
            #[cfg(not(target_os = "windows"))]
            let stopped_via_service = false;
            if !stopped_via_service
                && !kill_external(descriptor.pid, descriptor.binary_path.as_deref())
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
                        candidates.push(dir.join(crate::core_download::core_bin_file(name)));
                    }
                }
            }
            if let Some(data_dir) = self.shared.work_dir.parent() {
                let tb = data_dir.join("tun-binaries");
                if let Ok(versions) = std::fs::read_dir(&tb) {
                    for v in versions.flatten() {
                        for name in ["mihomo", "sing-box"] {
                            candidates.push(v.path().join(crate::core_download::core_bin_file(name)));
                        }
                    }
                }
                for name in ["mihomo", "sing-box"] {
                    candidates.push(data_dir.join(crate::core_download::core_bin_file(name)));
                }
            }
            candidates.sort();
            candidates.dedup();
            let mut note = String::new();
            for (pid, path) in find_leftover_core_pids(&candidates) {
                if kill_external(pid, Some(&path)) {
                    note.push_str(&format!("已清理本应用残留内核 pid={pid}（{}）；", path.display()));
                } else {
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
                && escalate_clear_port(engine_rt.mixed_port)
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
                            note.push_str(&format!(
                                "已清理本应用残留内核 {image}（pid={pid}）；"
                            ));
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
                Duration::from_secs(30)
            };
            if let Err(e) = wait_for_mixed_port(&try_bind, handover_wait) {
                let listeners = port_listeners_via_lsof(engine_rt.mixed_port);
                for (pid, cmd) in &listeners {
                    note.push_str(&format!("占用者 pid={pid}（{cmd}）；"));
                }
                return Err(format!(
                    "混合端口 {} 无法绑定（{e}）。{note}若仍失败：请更换本应用的混合端口，或退出占用该端口的程序",
                    engine_rt.mixed_port
                ));
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
            None => spawn_and_watch(self.shared.clone()),
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
                {
                    let mut inner = self.shared.inner.lock().unwrap();
                    inner.external_pid = Some(pid);
                    inner.detached = true;
                    #[cfg(target_os = "linux")]
                    {
                        inner.external_binary = Some(crate::linux_service::binary_path());
                    }
                }
                #[cfg(target_os = "linux")]
                let actual_binary = crate::linux_service::binary_path();
                #[cfg(not(target_os = "linux"))]
                let actual_binary = self.shared.binary_path.clone();
                if let Err(error) = self
                    .shared
                    .write_descriptor(pid, port, &secret, &actual_binary)
                {
                    eprintln!("could not persist core descriptor: {error}");
                }
                // SYSTEM 内核归服务管理，这里只探测置 Running（无子进程句柄
                // 可监视；死亡可感知性由 #20 健康监测的控制器探测兜底）
                let shared = self.shared.clone();
                std::thread::spawn(move || {
                    probe_external_core(shared, port, secret);
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
        if d.binary_path.as_ref() == Some(&crate::linux_service::binary_path()) {
            return false;
        }
        // Do not block GUI startup probing a descriptor whose process already exited
        // or whose PID was reused by a different binary.
        let gone = || !pid_is_alive(d.pid) || d.binary_path.as_ref()
            .is_some_and(|path| process_matches_binary(d.pid, path) == Some(false));
        if gone() {
            let _ = std::fs::remove_file(Shared::descriptor_path(&self.shared.work_dir));
            return false;
        }
        let probe = ControllerProbe {
            port: d.port,
            secret: d.secret.clone(),
        };
        let timeout = if cfg!(windows) { Duration::from_secs(2) } else { Duration::from_secs(15) };
        if !probe.wait_ready(timeout, || false, gone) {
            if kill_external(d.pid, d.binary_path.as_deref()) {
                let _ = std::fs::remove_file(Shared::descriptor_path(&self.shared.work_dir));
            }
            return false;
        }
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

    /// 优雅停止：标记后杀进程，watch 线程随之退出。
    pub fn stop(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.shared.inner.lock().unwrap_or_else(|e| e.into_inner()).stopping = true;
        }
        #[cfg(target_os = "linux")]
        match crate::linux_service::stop_core() {
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
                self.shared.notify(CoreStatus::Crashed(format!("Linux TUN 清理失败：{error}")));
                return false;
            }
        }
        // Windows TUN 内核以 SYSTEM 运行，GUI 杀不动：先请服务停
        #[cfg(target_os = "windows")]
        {
            if crate::service_client::stop_core().is_ok() {
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
fn spawn_and_watch(shared: Arc<Shared>) {
    shared.notify(CoreStatus::Starting);
    if let Err(e) = shared.spawn_child() {
        shared.notify(CoreStatus::Crashed(e));
        return;
    }
    let Some((port, secret)) = shared.inner.lock().unwrap().controller.clone() else {
        shared.notify(CoreStatus::Crashed("controller state missing".into()));
        return;
    };
    std::thread::spawn(move || watch(shared, port, secret));
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

/// root 内核补杀的授权弹窗门控：应用退出路径置 true（退出时不该突然弹
/// 管理员框），用户主动的 TUN/引擎/配置切换保持允许——这些本就是会弹
/// 授权框的语义场景。拒绝授权时停止流程照常继续（预绑定检查兜底报错）。
pub static ROOT_KILL_PROMPT_DISABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 杀掉不属于本进程的内核 pid（收养场景）。unix 用 SIGKILL，其余平台降级。
fn kill_pid(pid: u32) {
    #[cfg(unix)]
    unsafe {
        let _ = libc::kill(pid as i32, libc::SIGKILL);
    }
    #[cfg(not(unix))]
    {
        let _ = crate::process::command("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output();
    }
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
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_INVALID_PARAMETER, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{OpenProcess, GetExitCodeProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() { return GetLastError() != ERROR_INVALID_PARAMETER; }
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

/// root 内核对普通用户 SIGKILL 免疫（EPERM）：SIGKILL 后仍存活时，
/// 请管理员授权补杀。仅在确有 root 内核残留时才会弹授权框。
#[cfg(target_os = "macos")]
fn escalate_admin_kill(pid: u32) {
    let script = format!("kill -9 {pid}");
    let out = crate::process::command("osascript")
        .args(["-e", &privileged_shell_script(&script)])
        .output();
    if let Err(e) = out {
        eprintln!("admin kill {pid}: osascript failed: {e}");
    } else if !out.unwrap().status.success() {
        eprintln!("admin kill {pid}: user denied or failed");
    }
}

#[cfg(not(target_os = "macos"))]
fn escalate_admin_kill(_pid: u32) {}

/// 杀外部内核并确认真正退出；SIGKILL 无效（root 内核）时管理员补杀。
/// `expected`：进程身份校验路径（proc_pidpath / /proc/exe 比对）。
/// 缺少身份信息时只在 PID 已不存在的情况下报告完成，不发送信号或提权。
/// 返回是否确认已退出。
fn kill_external(pid: u32, expected: Option<&std::path::Path>) -> bool {
    let Some(expected) = expected else {
        if !pid_is_alive(pid) {
            return true;
        }
        eprintln!("core kill: refusing to signal unverified external pid {pid}");
        return false;
    };
    // A permissions error is not proof that a privileged process has exited.
    let gone = || process_matches_binary(pid, expected) == Some(false) || !pid_is_alive(pid);
    if gone() {
        return true;
    }
    if process_matches_binary(pid, expected) != Some(true) {
        eprintln!("core kill: cannot verify external process identity; retaining descriptor");
        return false;
    }
    kill_pid(pid);
    for _ in 0..40 {
        if gone() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // 2 秒仍存活：root 内核，SIGKILL 对普通用户 EPERM——授权补杀
    if !ROOT_KILL_PROMPT_DISABLED.load(std::sync::atomic::Ordering::SeqCst) {
        escalate_admin_kill(pid);
    }
    for _ in 0..40 {
        if gone() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    eprintln!("core kill: external pid {pid} still alive after admin kill");
    false
}

#[derive(serde::Deserialize)]
struct CoreDescriptor {
    port: u16,
    secret: String,
    pid: u32,
    #[serde(default)]
    binary_path: Option<PathBuf>,
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
            if candidates.iter().any(|c| *c == path) {
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
                    if let Some(matched) = candidates
                        .iter()
                        .find(|candidate| process_matches_binary(entry.th32ProcessID, candidate) == Some(true))
                    {
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
        let Some(last) = line.split_whitespace().last() else { continue };
        let Ok(pid) = last.parse::<u32>() else { continue };
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

#[cfg(not(target_os = "windows"))]
fn windows_port_listeners(_port: u16) -> Vec<(u32, String)> {
    Vec::new()
}

/// 端口被占且普通权限查不到占用者（root 内核对非 root 完全不可见：
/// lsof/proc_pidpath 全瞎）时，用管理员权限在 root 上下文里定位端口
/// 监听者，且仅当其命令行含内核特征时才杀。返回是否清理成功。
#[cfg(target_os = "macos")]
fn escalate_clear_port(port: u16) -> bool {
    // 脚本刻意不含双引号与反斜杠，免去 AppleScript 字符串转义；
    // 仅当监听者命令行含内核特征时才杀，避免误伤其他程序。
    let shell = format!(
        "pids=$(lsof -t -n -i TCP:{port} -sTCP:LISTEN 2>/dev/null); killed=0; for p in $pids; do c=$(ps -p $p -o command=); case $c in *com.crossbow.app/tun-binaries/*|*com.richard.crossbow/tun-binaries/*|*/Crossbow.app/Contents/MacOS/mihomo*|*/Crossbow.app/Contents/MacOS/sing-box*) kill -9 $p && killed=1;; esac; done; test $killed -eq 1"
    );
    let apple = privileged_shell_script(&shell);
    let out = crate::process::command("osascript")
        .arg("-e")
        .arg(apple)
        .output();
    match out {
        Ok(o) if o.status.success() => true,
        Ok(o) => {
            eprintln!(
                "admin clear port {port}: denied or failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            false
        }
        Err(e) => {
            eprintln!("admin clear port {port}: {e}");
            false
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn escalate_clear_port(_port: u16) -> bool {
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
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(3));
        {
            let inner = shared.inner.lock().unwrap_or_else(|e| e.into_inner());
            if inner.stopping || inner.external_pid != Some(pid) { return; }
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
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(1));
        {
            let inner = shared.inner.lock().unwrap_or_else(|e| e.into_inner());
            if inner.stopping || inner.external_pid != Some(pid) { return; }
        }
        let result = crate::linux_service::status();
        if result.as_ref().ok().and_then(|s| s.as_ref())
            .is_some_and(|s| s.pid == pid && s.active) { continue; }
        {
            let inner = shared.inner.lock().unwrap_or_else(|e| e.into_inner());
            if inner.stopping || inner.external_pid != Some(pid) { return; }
        }
        let message = match result {
            Ok(_) => "Linux TUN 服务已停止，接管资源已释放".to_string(),
            Err(error) => format!("Linux TUN 服务失联，尚未确认清理完成：{error}"),
        };
        // Keep the tracked PID on uncertain cleanup so a later stop can retry.
        shared.notify(CoreStatus::Crashed(message));
        return;
    });
}

/// 服务托管内核的就绪探测（watch() 的无子进程变体）：置 Running 或 Crashed。
fn probe_external_core(shared: Arc<Shared>, port: u16, secret: String) {
    let timeout = Duration::from_secs(shared.opts.lock().unwrap().readiness_timeout_secs);
    let probe = ControllerProbe { port, secret };
    if probe.wait_ready(
        timeout,
        || shared.inner.lock().unwrap().stopping,
        || false,
    ) {
        if shared.inner.lock().unwrap().stopping {
            return;
        }
        shared.notify(CoreStatus::Running);
    } else {
        if shared.inner.lock().unwrap().stopping {
            return;
        }
        shared.notify(CoreStatus::Crashed("服务托管内核启动失败/超时".into()));
    }
}

fn watch(shared: Arc<Shared>, port: u16, secret: String) {
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
            Some(c) => c.try_wait().ok().flatten().is_some(),
            None => true,
        }
    };
    if !probe.wait_ready(
        Duration::from_secs(opts.readiness_timeout_secs),
        || shared.inner.lock().unwrap().stopping,
        child_gone,
    ) {
        let stopping = shared.inner.lock().unwrap().stopping;
        // 无论打断原因，先收走子进程（stop 打断路径若不杀，刚拉起的内核会变孤儿）
        shared.kill_child();
        if stopping {
            // 收尾已由 stop() 上报 Stopped，这里静默退出
            return;
        }
        let summary = core_error_summary(&shared.work_dir, 200)
            .unwrap_or_else(|| {
                format!(
                    "就绪探针 {}s 未通过，内核无错误输出（详见监测→日志）",
                    opts.readiness_timeout_secs
                )
            });
        shared.notify(CoreStatus::Crashed(format!(
            "内核启动失败/超时：{summary}"
        )));
        return;
    }
    shared.notify(CoreStatus::Running);

    // 子进程始终留在共享状态（stop/pid 都依赖它）；这里只轮询退出状态。
    loop {
        let exit = {
            let mut inner = shared.inner.lock().unwrap();
            match inner.child.as_mut() {
                Some(c) => c.try_wait().ok().flatten(),
                None => return, // stop() 已收走并上报
            }
        };
        let Some(exit) = exit else {
            std::thread::sleep(Duration::from_millis(200));
            continue;
        };
        if shared.inner.lock().unwrap().stopping {
            return; // stop() 负责上报 Stopped
        }
        let summary = core_error_summary(&shared.work_dir, 200)
            .unwrap_or_else(|| "内核无错误输出（详见监测→日志）".into());
        let reason = match exit.code() {
            Some(code) => format!("内核异常退出（code {code}）：{summary}"),
            None => format!("内核被信号终止：{summary}"),
        };
        shared.notify(CoreStatus::Crashed(reason));

        // 崩溃自动重启：窗口限次，退避 800ms。
        if allow_restart(&shared) {
            std::thread::sleep(Duration::from_millis(800));
            if !shared.inner.lock().unwrap().stopping {
                {
                    let mut inner = shared.inner.lock().unwrap();
                    inner.detached = false;
                    inner.external_pid = None;
                    inner.external_binary = None;
                }
                spawn_and_watch(shared);
            }
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

    use super::*;

    #[test]
    fn dead_descriptor_is_removed_without_waiting_for_controller_timeout() {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        let mut child = crate::process::command("sh").args(["-c", "exit 0"]).spawn().unwrap();
        #[cfg(windows)]
        let mut child = crate::process::command("cmd.exe").args(["/C", "exit", "0"]).spawn().unwrap();
        let pid = child.id(); child.wait().unwrap();
        let manager = CoreManager::new(PathBuf::from("missing-core"), dir.path().into());
        manager.shared.write_descriptor(pid, free_port().unwrap(), "test-secret", Path::new("missing-core")).unwrap();
        let started = Instant::now();
        assert!(!manager.adopt_external());
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(!Shared::descriptor_path(dir.path()).exists());
    }

    #[cfg(windows)]
    #[test]
    fn windows_liveness_detects_exit_with_child_handle_still_retained() {
        let mut child = crate::process::command("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", "Start-Sleep -Seconds 5"])
            .spawn().unwrap();
        let pid = child.id();
        assert!(pid_is_alive(pid));
        child.kill().unwrap(); child.wait().unwrap();
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

    /// Network-free integration: real mihomo, private ports and temporary files.
    #[test]
    fn real_core_reload_preserves_pid_and_restores_rejected_config() {
        let _serial = REAL_CORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(bin) = std::env::var("CROSSBOW_CORE_BIN") else { return; };
        let dir = tempfile::tempdir().unwrap();
        let mgr = CoreManager::new(PathBuf::from(bin), dir.path().to_path_buf());
        mgr.set_options(CoreOptions { ensure_geo_files: false, ..CoreOptions::default() });
        let rt = RuntimeConfig { mixed_port: free_port().unwrap(), ..RuntimeConfig::default() };
        let base = "proxies: []\nrules: ['MATCH,DIRECT']\n";
        mgr.start(base, &rt).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while mgr.status() != CoreStatus::Running {
            assert!(Instant::now() < deadline, "core not ready: {:?}", mgr.status());
            std::thread::sleep(Duration::from_millis(50));
        }
        let pid = mgr.pid();
        let (port, secret) = mgr.controller().unwrap();
        let controller = crate::mihomo_api::Controller { port, secret: secret.clone() };
        let old = mgr.current_config().unwrap();
        let new_config = old.replace("mode: rule", "mode: direct");
        assert_ne!(old, new_config);
        mgr.write_config_file(&new_config).unwrap();
        controller.reload_config(&mgr.config_path()).unwrap();
        mgr.replace_cached_config(new_config.clone());
        assert_eq!(mgr.pid(), pid);
        assert_eq!(mgr.controller(), Some((port, secret)));
        assert_eq!(mgr.current_config(), Some(new_config.clone()));
        mgr.write_config_file("proxies: [invalid-proxy-entry]").unwrap();
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

        mgr.stop();
        assert_eq!(mgr.status(), CoreStatus::Stopped);
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
