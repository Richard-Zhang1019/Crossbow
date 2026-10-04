//! 内核 sidecar 生命周期管理：启动、就绪探测、停止、崩溃自动重启。
//!
//! 刻意不依赖 Tauri，便于直接单元测试与集成测试（真实内核冒烟）。
//! 平台注意：Windows 下 spawn 需带 CREATE_NO_WINDOW，避免 GUI 进程闪控制台。

use std::collections::VecDeque;
use std::io::Write;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command};
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

impl Drop for Shared {
    fn drop(&mut self) {
        // 兜底防孤儿：最后一个管理器句柄丢弃（含 panic 路径）时收走内核进程。
        self.inner.lock().unwrap().stopping = true;
        self.kill_child();
    }
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
        {
            let mut inner = self.shared.inner.lock().unwrap();
            inner.stopping = true;
            inner.controller = None;
        }
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
    let Some((port, secret)) = shared.inner.lock().unwrap().controller.clone() else {
        shared.notify(CoreStatus::Crashed("controller state missing".into()));
        return;
    };
    std::thread::spawn(move || watch(shared, port, secret));
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
    if !probe.wait_ready(Duration::from_secs(opts.readiness_timeout_secs)) {
        shared.kill_child();
        shared.notify(CoreStatus::Crashed(format!(
            "内核启动失败/超时：{}",
            core_log_tail(&shared.work_dir, 400)
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
        let reason = match exit.code() {
            Some(code) => format!(
                "内核异常退出（code {code}）：{}",
                core_log_tail(&shared.work_dir, 400)
            ),
            None => format!("内核被信号终止：{}", core_log_tail(&shared.work_dir, 400)),
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

/// GeoIP/GeoSite 数据源：镜像优先，直连 GitHub 兜底。
/// 镜像失效时调整此列表即可（均为数据文件，不参与代码构建）。
pub const GEO_FILES: &[(&str, bool, &[&str])] = &[
    (
        "geoip.metadb",
        true,
        &[
            // 2026-10 在 CN 网络实测可用
            "https://gh-proxy.com/https://github.com/MetaCubeX/meta-rules-dat/releases/latest/download/geoip.metadb",
            "https://github.com/MetaCubeX/meta-rules-dat/releases/latest/download/geoip.metadb",
        ],
    ),
    (
        "geosite.dat",
        false, // 尽力而为：仅当规则用到 GEOSITE 时内核才需要
        &[
            "https://gh-proxy.com/https://github.com/MetaCubeX/meta-rules-dat/releases/latest/download/geosite.dat",
            "https://github.com/MetaCubeX/meta-rules-dat/releases/latest/download/geosite.dat",
        ],
    ),
];

const GEO_MIN_BYTES: u64 = 1_000_000;

/// 确保内核工作目录里有可用的 geo 数据文件；缺失/过小时逐镜像下载。
fn ensure_geo_files(work_dir: &std::path::Path) -> Result<(), String> {
    ensure_geo_files_with(work_dir, GEO_FILES, download_to)
}

fn ensure_geo_files_with(
    work_dir: &std::path::Path,
    files: &[(&str, bool, &[&str])],
    download: impl Fn(&str, &std::path::Path) -> Result<(), String>,
) -> Result<(), String> {
    for (name, required, _) in files {
        let dest = work_dir.join(name);
        if dest.exists() && dest.metadata().map(|m| m.len()).unwrap_or(0) >= GEO_MIN_BYTES {
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
            Err(e) => return Err(format!("{name}: {e}")),
        }
    }
    Ok(())
}

/// 依次尝试 GeoFiles 里该文件的镜像源；下载后做最小健全性检查（大小 + metadb 头）。
fn download_to(name: &str, dest: &std::path::Path) -> Result<(), String> {
    let urls = GEO_FILES
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, _, urls)| *urls)
        .ok_or_else(|| format!("unknown geo file {name}"))?;
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
                // 健全性检查：足够大，且 metadb 头为 00 00 01 xx
                let ok_size =
                    std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0) >= GEO_MIN_BYTES;
                let head_ok = name.ends_with("metadb") && {
                    let mut head = [0u8; 2];
                    std::fs::File::open(&tmp)
                        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut head))
                        .is_ok_and(|_| head[0] == 0 && head[1] == 0)
                } || !name.ends_with("metadb");
                if !ok_size || !head_ok {
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

/// 读取内核输出日志的尾部（崩溃原因展示用）。
fn core_log_tail(work_dir: &std::path::Path, max_chars: usize) -> String {
    let path = work_dir.join("core-stdio.log");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return "（无内核输出）".into();
    };
    let trimmed: String = raw.trim().to_string();
    if trimmed.is_empty() {
        return "（内核无输出）".into();
    }
    let char_count = trimmed.chars().count();
    if char_count <= max_chars {
        return trimmed;
    }
    let skip = char_count - max_chars;
    format!("…{}", trimmed.chars().skip(skip).collect::<String>())
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
            ensure_geo_files: false,
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
