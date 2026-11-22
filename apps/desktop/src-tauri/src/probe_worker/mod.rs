pub(crate) mod config;
pub(crate) mod guardian;
pub(crate) mod http;
pub(crate) mod transport;
use crate::node_probes::Budget;
use crossbow_core::Engine;
use std::{
    net::TcpListener,
    path::Path,
    process::{Child, Stdio},
    time::{Duration, Instant},
};

/// Only the child owned by this guard is terminated. TempDir drops after the process exits.
pub(crate) struct Worker {
    child: std::sync::Arc<std::sync::Mutex<Child>>,
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    monitor: Option<std::thread::JoinHandle<()>>,
    _dir: tempfile::TempDir,
    pub port: u16,
    pub password: String,
    pub udp_supported: bool,
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.done.store(true, std::sync::atomic::Ordering::SeqCst);
        {
            let mut child = self.child.lock().unwrap_or_else(|e| e.into_inner());
            drop(child.stdin.take());
            let deadline = Instant::now() + Duration::from_secs(2);
            while child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            #[cfg(unix)]
            {
                // SAFETY: this process group was created exclusively for this owned guardian and its core.
                unsafe {
                    libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
                }
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(monitor) = self.monitor.take() {
            let _ = monitor.join();
        }
    }
}
impl Worker {
    pub fn start(
        binary: &Path,
        text: &str,
        engine: Engine,
        node: &str,
        budget: &Budget,
    ) -> Result<Self, String> {
        budget.check()?;
        let reservation = TcpListener::bind("127.0.0.1:0").map_err(|_| "无法预留测试端口")?;
        let port = reservation
            .local_addr()
            .map_err(|_| "无法读取测试端口")?
            .port();
        let password = uuid::Uuid::new_v4().simple().to_string();
        let config = config::build(text, engine, node, port, &password)?;
        let dir = tempfile::Builder::new()
            .prefix("crossbow-probe-")
            .tempdir()
            .map_err(|_| "无法创建私有测试目录")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
                .map_err(|_| "无法保护测试目录")?;
        }
        let path = dir.path().join("probe.json");
        crate::platform::fs_perms::write_private(
            &path,
            &serde_json::to_vec(&config.document).map_err(|_| "无法生成测试配置")?,
        )
        .map_err(|_| "无法保存私有测试配置")?;
        let marker = guardian::Launch {
            version: 1,
            binary: binary.to_path_buf(),
            mihomo: engine == Engine::Mihomo,
        };
        crate::platform::fs_perms::write_private(
            &dir.path().join("launch.json"),
            &serde_json::to_vec(&marker).map_err(|_| "无法生成测试启动描述")?,
        )
        .map_err(|_| "无法保存测试启动描述")?;
        let helper = std::env::current_exe().map_err(|_| "无法定位测试守护程序")?;
        #[cfg(test)]
        let helper = std::env::var_os("CB_PROBE_HELPER")
            .map(std::path::PathBuf::from)
            .unwrap_or(helper);
        let mut command = crate::process::command(helper);
        command
            .arg(guardian::ARG)
            .arg(dir.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        drop(reservation);
        budget.check()?;
        let child = command.spawn().map_err(|_| "无法启动隔离测试内核")?;
        let child = std::sync::Arc::new(std::sync::Mutex::new(child));
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watched = child.clone();
        let finished = done.clone();
        let guard = budget.clone();
        let monitor = std::thread::spawn(move || {
            while !finished.load(std::sync::atomic::Ordering::SeqCst) {
                if guard.check().is_err() {
                    drop(
                        watched
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .stdin
                            .take(),
                    );
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let worker = Self {
            child,
            done,
            monitor: Some(monitor),
            _dir: dir,
            port,
            password,
            udp_supported: config.udp_supported,
        };
        let ready = Instant::now() + Duration::from_secs(5);
        while Instant::now() < ready {
            budget.check()?;
            if worker
                .child
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .try_wait()
                .map_err(|_| "无法检查测试内核")?
                .is_some()
            {
                return Err("隔离测试内核启动失败；该节点参数或依赖不支持测试配置".into());
            }
            if transport::authenticate(
                port,
                &worker.password,
                &budget.child(Duration::from_millis(300)),
            )
            .is_ok()
            {
                return Ok(worker);
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        Err("隔离测试内核启动超时".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    /// Set CB_PROBE_CORE and CB_PROBE_ENGINE; no real subscriptions or host-network mutations.
    #[test]
    #[ignore = "requires explicit isolated native core fixture"]
    fn native_tcp_success_https_failure_and_cancel_cleanup() {
        use std::io::{Read, Write};
        let binary = std::env::var("CB_PROBE_CORE").expect("CB_PROBE_CORE");
        let engine = if std::env::var("CB_PROBE_ENGINE").as_deref() == Ok("singbox") {
            Engine::SingBox
        } else {
            Engine::Mihomo
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let upstream_port = listener.local_addr().unwrap().port();
        let done = Arc::new(AtomicBool::new(false));
        let stop = done.clone();
        let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let connections = accepted.clone();
        let server = std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(15);
            while !stop.load(Ordering::SeqCst) && Instant::now() < until {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(1)))
                            .unwrap();
                        let mut request = [0; 4096];
                        let n = stream.read(&mut request).unwrap_or(0);
                        if request[..n].starts_with(b"CONNECT ") {
                            connections.fetch_add(1, Ordering::SeqCst);
                        }
                        let _ = stream.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n");
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        let source = if engine == Engine::Mihomo {
            serde_json::json!({"proxies":[{"name":"fixture","type":"http","server":"127.0.0.1","port":upstream_port}]})
        } else {
            serde_json::json!({"outbounds":[{"tag":"fixture","type":"http","server":"127.0.0.1","server_port":upstream_port}]})
        };
        let current = Arc::new(AtomicBool::new(true));
        let valid = current.clone();
        let budget = Budget::new(Arc::new(move || valid.load(Ordering::SeqCst)));
        let worker = Worker::start(
            Path::new(&binary),
            &source.to_string(),
            engine,
            "fixture",
            &budget,
        )
        .unwrap();
        let child = worker.child.clone();
        let directory = worker._dir.path().to_path_buf();
        let tcp = transport::tcp(
            worker.port,
            &worker.password,
            &budget.child(Duration::from_secs(2)),
        );
        let https = http::https(
            worker.port,
            &worker.password,
            &budget.child(Duration::from_secs(3)),
        );
        assert_eq!(tcp.class, "ok");
        assert_eq!(https.class, "https_unverified");
        assert_eq!(
            accepted.load(Ordering::SeqCst),
            2,
            "HTTPS must reach the actual fixture, not fail at local proxy authentication"
        );
        assert!(!worker.udp_supported);
        current.store(false, Ordering::SeqCst);
        let start = Instant::now();
        while child.lock().unwrap().try_wait().unwrap().is_none()
            && start.elapsed() < Duration::from_secs(2)
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            child.lock().unwrap().try_wait().unwrap().is_some(),
            "watchdog must kill only the owned child promptly"
        );
        drop(worker);
        assert!(!directory.exists());
        done.store(true, Ordering::SeqCst);
        server.join().unwrap();
        println!("native evidence: engine={engine:?}, TCP={}, HTTPS={}, HTTP-node UDP=unsupported, cancellation_ms={}, private_directory_removed=true",tcp.class,https.class,start.elapsed().as_millis());
    }
}
