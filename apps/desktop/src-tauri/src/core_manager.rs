//! 内核 sidecar 生命周期管理：启动、就绪探测、停止、崩溃自动重启。
//!
//! 刻意不依赖 Tauri，便于直接单元测试与集成测试（真实内核冒烟）。
//! 平台注意：Windows 下 spawn 需带 CREATE_NO_WINDOW，避免 GUI 进程闪控制台。

use std::collections::VecDeque;
use std::io::Write;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
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
    pub readiness_timeout_secs: u64,
    /// 崩溃重启的统计窗口（秒）。
    pub restart_window_secs: u64,
    /// 窗口内最大重启次数，超过后放弃并保持 Crashed。
    pub max_restarts: u32,
}

impl Default for CoreOptions {
    fn default() -> Self {
        Self {
            readiness_timeout_secs: 15,
            restart_window_secs: 60,
            max_restarts: 3,
        }
    }
}

type Callback = Arc<dyn Fn(&CoreStatus) + Send + Sync>;

struct Inner {
    child: Option<Child>,
    status: CoreStatus,
    /// 手动停止标记：watch 线程看到退出时不重启。
    stopping: bool,
    restarts: VecDeque<Instant>,
    /// (port, secret)
    controller: Option<(u16, String)>,
}

/// 线程间共享状态；CoreManager 可 Clone。
struct Shared {
    binary_path: PathBuf,
    /// 内核工作目录（mihomo `-d`），config.yaml 写在这里。
    work_dir: PathBuf,
    opts: Mutex<CoreOptions>,
    callback: Mutex<Option<Callback>>,
    inner: Mutex<Inner>,
}

impl Shared {
    fn notify(&self, status: CoreStatus) {
        self.inner.lock().unwrap().status = status.clone();
        if let Some(cb) = self.callback.lock().unwrap().clone() {
            cb(&status);
        }
    }

    fn spawn_child(&self) -> Result<(), String> {
        let config_path = self.work_dir.join("config.yaml");
        let mut cmd = Command::new(&self.binary_path);
        cmd.arg("-d")
            .arg(&self.work_dir)
            .arg("-f")
            .arg(&config_path);
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let child = cmd.spawn().map_err(|e| format!("spawn core: {e}"))?;
        self.inner.lock().unwrap().child = Some(child);
        Ok(())
    }

    fn kill_child(&self) {
        if let Some(mut child) = self.inner.lock().unwrap().child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

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
                inner: Mutex::new(Inner {
                    child: None,
                    status: CoreStatus::Stopped,
                    stopping: false,
                    restarts: VecDeque::new(),
                    controller: None,
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
        self.shared.binary_path.clone()
    }

    /// 内核子进程 PID；诊断信息展示与测试用。
    #[allow(dead_code)]
    pub fn pid(&self) -> Option<u32> {
        self.shared
            .inner
            .lock()
            .unwrap()
            .child
            .as_ref()
            .map(|c| c.id())
    }

    /// 启动内核。`base_config` 是渲染后的订阅配置（未经运行时注入）。
    pub fn start(&self, base_config: &str, engine_rt: &RuntimeConfig) -> Result<(), String> {
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
        std::fs::create_dir_all(&self.shared.work_dir).map_err(|e| e.to_string())?;

        let port = free_port().ok_or("no free port for controller")?;
        let secret = Uuid::new_v4().simple().to_string();
        let rt = RuntimeConfig {
            controller_port: port,
            controller_secret: secret.clone(),
            ..engine_rt.clone()
        };
        let final_config = apply_runtime(base_config, &rt).map_err(|e| e.to_string())?;
        let config_path = self.shared.work_dir.join("config.yaml");
        std::fs::File::create(&config_path)
            .and_then(|mut f| f.write_all(final_config.as_bytes()))
            .map_err(|e| format!("write config: {e}"))?;

        {
            let mut inner = self.shared.inner.lock().unwrap();
            inner.stopping = false;
            inner.controller = Some((port, secret));
            inner.restarts.clear();
        }
        spawn_and_watch(self.shared.clone());
        Ok(())
    }

    /// 优雅停止：标记后杀进程，watch 线程随之退出。
    pub fn stop(&self) {
        self.shared.inner.lock().unwrap().stopping = true;
        self.shared.kill_child();
        self.shared.notify(CoreStatus::Stopped);
    }
}

/// 拉起子进程 + 就绪探测 + 监视线程（重启时复用，不重写配置文件）。
fn spawn_and_watch(shared: Arc<Shared>) {
    shared.notify(CoreStatus::Starting);
    if let Err(e) = shared.spawn_child() {
        shared.notify(CoreStatus::Crashed(e));
        return;
    }
    let (port, secret) = shared.inner.lock().unwrap().controller.clone().unwrap();
    std::thread::spawn(move || watch(shared, port, secret));
}

impl Drop for CoreManager {
    fn drop(&mut self) {
        // 兜底防孤儿：管理器被丢弃（含 panic 路径）时必须收走内核进程。
        self.stop();
    }
}

fn watch(shared: Arc<Shared>, port: u16, secret: String) {
    let opts = shared.opts.lock().unwrap().clone();
    let probe = ControllerProbe { port, secret };
    if !probe.wait_ready(Duration::from_secs(opts.readiness_timeout_secs)) {
        shared.kill_child();
        shared.notify(CoreStatus::Crashed("readiness timeout".into()));
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
        let reason = match exit.code() {
            Some(code) => format!("core exited with code {code}"),
            None => "core terminated by signal".into(),
        };
        shared.notify(CoreStatus::Crashed(reason));

        // 崩溃自动重启：窗口限次，退避 800ms。
        if allow_restart(&shared) {
            std::thread::sleep(Duration::from_millis(800));
            if !shared.inner.lock().unwrap().stopping {
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
    /// 轮询 `/version` 直到就绪或超时。
    fn wait_ready(&self, timeout: Duration) -> bool {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_millis(800))
            .build();
        let url = format!("http://127.0.0.1:{}/version", self.port);
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(resp) = agent
                .get(&url)
                .set("Authorization", &format!("Bearer {}", self.secret))
                .call()
            {
                if resp.status() == 200 {
                    return true;
                }
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        false
    }
}

/// 分配一个空闲 TCP 端口（先绑定 0 拿端口再释放，存在极小竞态，S2 可接受）。
pub fn free_port() -> Option<u16> {
    let l = TcpListener::bind(("127.0.0.1", 0)).ok()?;
    l.local_addr().ok().map(|a| a.port())
}

#[cfg(test)]
mod tests {
    use super::*;

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
            ..CoreOptions::default()
        });
        let _ = mgr.start("", &RuntimeConfig::default());
        for _ in 0..80 {
            if matches!(mgr.status(), CoreStatus::Crashed(_)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("expected Crashed, got {:?}", mgr.status());
    }

    /// 真实内核冒烟：CROSSBOW_CORE_BIN 指向 mihomo 时才运行。
    /// 验证 启动 → Running → 混合端口可代理 → 停止 → Stopped。
    #[test]
    fn real_core_lifecycle_and_proxy_roundtrip() {
        let Ok(bin) = std::env::var("CROSSBOW_CORE_BIN") else {
            eprintln!("skip: CROSSBOW_CORE_BIN not set");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let mgr = CoreManager::new(PathBuf::from(bin), dir.path().to_path_buf());
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

    /// 真实内核崩溃自动恢复：Running 中强杀子进程 → 应自动回到 Running。
    #[test]
    fn real_core_crash_auto_recovers() {
        let Ok(bin) = std::env::var("CROSSBOW_CORE_BIN") else {
            eprintln!("skip: CROSSBOW_CORE_BIN not set");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let mgr = CoreManager::new(PathBuf::from(bin), dir.path().to_path_buf());
        let mut o = CoreOptions::default();
        o.readiness_timeout_secs = 10;
        mgr.set_options(o);
        mgr.start(
            "proxies: []\nrules:\n  - MATCH,DIRECT\n",
            &RuntimeConfig::default(),
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
