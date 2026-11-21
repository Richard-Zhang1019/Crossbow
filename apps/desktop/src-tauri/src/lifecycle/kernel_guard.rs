//! Detached macOS guard. Its authority is one private, revocable session, tied
//! to an exact journal and core descriptor. It never starts or kills a core.
use crate::network::sysproxy::{NetworkSetup, ServiceSnapshot, SysProxyBackend, SysProxyManager};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

pub const GUARD_ARG: &str = "--kernel-guard";
const LABEL: &str = "com.richard.crossbow.kernel-guard";
const SESSION: &str = "kernel-guard-session.json";
const WATCH_TICK: Duration = Duration::from_secs(10);

#[derive(Serialize, Deserialize)]
struct Session {
    id: String,
    journal_hash: Vec<u8>,
    descriptor_hash: Vec<u8>,
    english: bool,
}
#[derive(Deserialize)]
struct Journal {
    port: u16,
    snapshot: BTreeMap<String, ServiceSnapshot>,
}
#[derive(Deserialize)]
struct Descriptor {
    pid: u32,
    port: u16,
    secret: String,
    binary_path: PathBuf,
}
#[derive(Debug, PartialEq, Eq)]
enum Health {
    Healthy,
    Unhealthy,
    Unknown,
}
#[derive(Debug, PartialEq, Eq)]
enum GuardAction {
    Yield,
    Watch,
    Restore,
}
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Yield,
    Watch,
    Recovered,
    Skipped,
}

fn decide(current: bool, gui_alive: bool, health: &Health, failures: u32) -> GuardAction {
    if !current || gui_alive {
        GuardAction::Yield
    } else if *health == Health::Unhealthy && failures >= 2 {
        GuardAction::Restore
    } else {
        GuardAction::Watch
    }
}
fn hash(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}
fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| e.to_string())
}
fn valid_journal(raw: &[u8]) -> Option<Journal> {
    serde_json::from_slice::<Journal>(raw)
        .ok()
        .filter(|j| j.port > 0 && !j.snapshot.is_empty())
}
impl Session {
    fn current(&self, dir: &Path) -> bool {
        read(&dir.join(SESSION))
            .ok()
            .and_then(|raw| serde_json::from_slice::<Session>(&raw).ok())
            .is_some_and(|other| other.id == self.id)
            && read(&dir.join("sysproxy-journal.json"))
                .is_ok_and(|raw| hash(&raw) == self.journal_hash && valid_journal(&raw).is_some())
            && read(&dir.join("runtime/core-descriptor.json"))
                .is_ok_and(|raw| hash(&raw) == self.descriptor_hash)
    }
}
fn core_health(dir: &Path) -> Health {
    let Some(d) = read(&dir.join("runtime/core-descriptor.json"))
        .ok()
        .and_then(|raw| serde_json::from_slice::<Descriptor>(&raw).ok())
    else {
        return Health::Unknown;
    };
    if d.pid == 0 || d.pid > i32::MAX as u32 || d.port == 0 || !d.binary_path.is_absolute() {
        return Health::Unknown;
    }
    if !crate::engine::manager::pid_is_alive(d.pid) {
        return Health::Unhealthy;
    }
    match crate::engine::manager::process_matches_binary(d.pid, &d.binary_path) {
        Some(false) => Health::Unhealthy,
        None => Health::Unknown,
        Some(true) => {
            let port = read(&dir.join("sysproxy-journal.json"))
                .ok()
                .and_then(|raw| valid_journal(&raw))
                .map(|j| j.port);
            if port.is_some_and(crate::lifecycle::exit_flow::port_has_listener)
                && crate::engine::manager::retained_controller_ready(d.pid, d.port, &d.secret)
            {
                Health::Healthy
            } else {
                Health::Unhealthy
            }
        }
    }
}
fn gui_alive(dir: &Path) -> bool {
    read(&dir.join("app.pid"))
        .ok()
        .and_then(|raw| std::str::from_utf8(&raw).ok()?.trim().parse::<u32>().ok())
        .filter(|&pid| pid > 0 && pid <= i32::MAX as u32)
        .is_some_and(crate::engine::manager::pid_is_alive)
}
fn plist_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .map(|home| home.join(format!("Library/LaunchAgents/{LABEL}.plist")))
}

fn recovery_file(dir: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join("kernel-guard-recovery.lock"))
}
fn recovering(dir: &Path) -> bool {
    recovery_file(dir).is_ok_and(|file| matches!(file.try_lock(), Err(TryLockError::WouldBlock)))
}
/// GUI startup waits only for an active guard recovery, not for another GUI.
pub(crate) fn acquire_gui_instance(dir: &Path) -> std::io::Result<File> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match crate::lock_app_instance(dir) {
            Ok(lock) => {
                // A failed launch attempt must not revoke the detached session.
                // Take ownership after any existing recovery writer first.
                let _ = std::fs::remove_file(dir.join(SESSION));
                return Ok(lock);
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    && recovering(dir)
                    && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(50));
            }
            result => return result,
        }
    }
}

/// Restrict the existing recovery manager to unattended writes. PAC/WPAD is
/// foreign ownership even when dormant manual slots still contain our port.
struct GuardBackend<B> {
    inner: B,
    restored: Arc<AtomicUsize>,
}
impl<B: SysProxyBackend> SysProxyBackend for GuardBackend<B> {
    fn list_services(&self) -> Result<Vec<String>, String> {
        self.inner.list_services()
    }
    fn enable_service(&self, _: &str, _: u16) -> Result<(), String> {
        Err("guard cannot enable proxy".into())
    }
    fn disable_service(&self, _: &str) -> Result<(), String> {
        Err("guard requires a valid snapshot".into())
    }
    fn service_snapshot(&self, service: &str) -> Result<ServiceSnapshot, String> {
        let mut snapshot = self.inner.service_snapshot(service)?;
        if self.inner.uses_automatic_proxy(service, &snapshot)? {
            snapshot.web = None;
            snapshot.secure = None;
            snapshot.socks = None;
        }
        Ok(snapshot)
    }
    fn restore_service(&self, service: &str, snapshot: &ServiceSnapshot) -> Result<(), String> {
        let mut target = snapshot.clone();
        // Do not replace one dead local proxy with another from an old snapshot.
        for endpoint in [&mut target.web, &mut target.secure, &mut target.socks] {
            if endpoint.as_ref().is_some_and(|e| {
                e.host == "127.0.0.1" && !crate::lifecycle::exit_flow::port_has_listener(e.port)
            }) {
                *endpoint = None;
            }
        }
        self.inner.restore_service_unattended(service, &target)?;
        self.restored.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
fn recover<B: SysProxyBackend>(
    dir: &Path,
    session: &Session,
    backend: B,
    health: impl Fn() -> Health,
    report_result: impl Fn(&Result<Outcome, String>),
) -> Result<Outcome, String> {
    let recovery = recovery_file(dir).map_err(|e| {
        let error = e.to_string();
        report_result(&Err(error.clone()));
        error
    })?;
    match recovery.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(Outcome::Watch),
        Err(error) => {
            let error = error.to_string();
            report_result(&Err(error.clone()));
            return Err(error);
        }
    }
    let _instance = match crate::lock_app_instance(dir) {
        Ok(lock) => lock,
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(Outcome::Watch),
        Err(e) => {
            let error = e.to_string();
            report_result(&Err(error.clone()));
            return Err(error);
        }
    };
    if !session.current(dir) || gui_alive(dir) {
        return Ok(Outcome::Yield);
    }
    if health() != Health::Unhealthy {
        return Ok(Outcome::Watch);
    }
    if !session.current(dir) || gui_alive(dir) {
        return Ok(Outcome::Yield);
    }
    let restored = Arc::new(AtomicUsize::new(0));
    let manager = SysProxyManager::new(
        GuardBackend {
            inner: backend,
            restored: restored.clone(),
        },
        dir.join("sysproxy-journal.json"),
    );
    let result = manager.disable().map(|()| {
        if restored.load(Ordering::SeqCst) > 0 {
            Outcome::Recovered
        } else {
            Outcome::Skipped
        }
    });
    // Persist/publish the result before releasing the GUI handoff lock.
    report_result(&result);
    result
}

fn english(dir: &Path) -> bool {
    read(&dir.join("store.json"))
        .ok()
        .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok())
        .is_some_and(|value| value.pointer("/ui/lang").and_then(|v| v.as_str()) == Some("en"))
}
fn report(dir: &Path, english: bool, recovered: bool, detail: String) {
    use crate::notifications::{self, Kind, Notice};
    let mut notice = Notice::new(
        if recovered {
            Kind::Warning
        } else {
            Kind::Error
        },
        "kernel.guard",
        "kernel-guard",
    );
    notice.title = if english {
        "Crossbow core guard"
    } else {
        "Crossbow 内核守护"
    }
    .into();
    notice.description = match (english, recovered) {
        (true, true) => "The retained core stopped responding. Owned proxy settings were restored from the saved snapshot. Reopen Crossbow to manage the core.",
        (false, true) => "保留的内核已停止响应，仍归 Crossbow 的代理设置已按原快照恢复。请重新打开 Crossbow 管理内核。",
        (true, false) => "Core guard setup or recovery failed. Reopen Crossbow and check system proxy settings.",
        (false, false) => "内核守护注册或恢复失败，请重新打开 Crossbow 并检查系统代理设置。",
    }.into();
    notice.title_key.clear();
    notice.details = detail;
    notifications::publish_detached(dir, notice);
}

pub(crate) fn register_and_start(dir: &Path) {
    if let Err(error) = register(dir) {
        eprintln!("kernel-guard registration failed: {error}");
        report(dir, english(dir), false, error);
    }
}
fn register(dir: &Path) -> Result<(), String> {
    let journal = match read(&dir.join("sysproxy-journal.json")) {
        Ok(raw) => raw,
        Err(_) if !dir.join("sysproxy-journal.json").exists() => return Ok(()),
        Err(error) => return Err(error),
    };
    valid_journal(&journal).ok_or("guard requires a valid nonempty proxy snapshot")?;
    let descriptor = read(&dir.join("runtime/core-descriptor.json"))?;
    let core: Descriptor = serde_json::from_slice(&descriptor).map_err(|e| e.to_string())?;
    if core.pid == 0
        || core.pid > i32::MAX as u32
        || !core.binary_path.is_absolute()
        || crate::engine::manager::process_matches_binary(core.pid, &core.binary_path) != Some(true)
    {
        return Err("cannot verify retained core identity".into());
    }
    let session = Session {
        id: uuid::Uuid::new_v4().to_string(),
        journal_hash: hash(&journal),
        descriptor_hash: hash(&descriptor),
        english: english(dir),
    };
    crate::platform::fs_perms::write_private(
        &dir.join(SESSION),
        &serde_json::to_vec(&session).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let result = (|| {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let plist = plist_path().ok_or("HOME is missing or not absolute")?;
        std::fs::create_dir_all(plist.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(dir.join("logs")).map_err(|e| e.to_string())?;
        let log = dir.join("logs/kernel-guard.log");
        crate::platform::fs_perms::open_private_truncate(&log).map_err(|e| e.to_string())?;
        let contents = plist_body(&exe, dir, &session.id, &log);
        crate::platform::fs_perms::write_private(&plist, contents.as_bytes())
            .map_err(|e| e.to_string())?;
        let domain = format!("gui/{}", unsafe { libc::getuid() });
        let _ = launchctl(&["bootout", &format!("{domain}/{LABEL}")]);
        launchctl(&[
            "bootstrap",
            &domain,
            plist.to_str().ok_or("non-UTF8 plist path")?,
        ])?;
        launchctl(&["kickstart", &format!("{domain}/{LABEL}")])
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(dir.join(SESSION));
    }
    result
}
pub(crate) fn unregister() {
    let _ = launchctl(&[
        "bootout",
        &format!("gui/{}/{LABEL}", unsafe { libc::getuid() }),
    ]);
    if let Some(path) = plist_path() {
        let _ = std::fs::remove_file(path);
    }
}
fn launchctl(args: &[&str]) -> Result<(), String> {
    run_command(
        std::process::Command::new("/bin/launchctl").args(args),
        Duration::from_secs(5),
    )
}
fn run_command(command: &mut std::process::Command, timeout: Duration) -> Result<(), String> {
    let mut child = command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if status.success() {
                    Ok(())
                } else {
                    Err(format!("launchctl exited {status}"))
                }
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            other => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "launchctl timed out or could not be observed: {other:?}"
                ));
            }
        }
    }
}
fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn plist_body(exe: &Path, dir: &Path, id: &str, log: &Path) -> String {
    let args = [
        exe.to_string_lossy().into_owned(),
        GUARD_ARG.into(),
        dir.to_string_lossy().into_owned(),
        id.into(),
    ]
    .iter()
    .map(|s| format!("<string>{}</string>", xml(s)))
    .collect::<String>();
    let log = xml(&log.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>Label</key><string>{LABEL}</string><key>ProgramArguments</key><array>{args}</array><key>StandardOutPath</key><string>{log}</string><key>StandardErrorPath</key><string>{log}</string></dict></plist>"#
    )
}

pub fn guard_main() -> ! {
    let mut args = std::env::args_os().skip(2);
    let dir = args.next().map(PathBuf::from).filter(|p| p.is_absolute());
    let id = args.next().and_then(|s| s.into_string().ok());
    let (Some(dir), Some(id)) = (dir, id) else {
        std::process::exit(2)
    };
    let Some(session) = read(&dir.join(SESSION))
        .ok()
        .and_then(|raw| serde_json::from_slice::<Session>(&raw).ok())
        .filter(|s| s.id == id)
    else {
        std::process::exit(0)
    };
    let mut failures = 0;
    loop {
        let health = core_health(&dir);
        failures = if health == Health::Unhealthy {
            failures + 1
        } else {
            0
        };
        match decide(session.current(&dir), gui_alive(&dir), &health, failures) {
            GuardAction::Yield => break,
            GuardAction::Watch => std::thread::sleep(WATCH_TICK),
            GuardAction::Restore => {
                match recover(
                    &dir,
                    &session,
                    NetworkSetup,
                    || core_health(&dir),
                    |result| match result {
                        Ok(Outcome::Recovered) => {
                            report(&dir, session.english, true, String::new())
                        }
                        Err(error) => report(&dir, session.english, false, error.clone()),
                        _ => {}
                    },
                ) {
                    Ok(Outcome::Watch) => std::thread::sleep(WATCH_TICK),
                    Ok(Outcome::Recovered) => {
                        break;
                    }
                    Ok(Outcome::Yield | Outcome::Skipped) => break,
                    Err(error) => {
                        eprintln!("kernel-guard recovery failed: {error}");
                        break;
                    }
                }
            }
        }
    }
    if read(&dir.join(SESSION))
        .ok()
        .and_then(|raw| serde_json::from_slice::<Session>(&raw).ok())
        .is_some_and(|s| s.id == id)
    {
        let _ = std::fs::remove_file(dir.join(SESSION));
    }
    std::process::exit(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::sysproxy::ProxyEndpoint;
    use std::sync::Mutex;

    #[derive(Clone)]
    struct Backend {
        snapshot: ServiceSnapshot,
        automatic: bool,
        fail: bool,
        writes: Arc<Mutex<Vec<ServiceSnapshot>>>,
    }
    impl SysProxyBackend for Backend {
        fn list_services(&self) -> Result<Vec<String>, String> {
            Ok(vec!["test".into()])
        }
        fn enable_service(&self, _: &str, _: u16) -> Result<(), String> {
            panic!("must not enable")
        }
        fn disable_service(&self, _: &str) -> Result<(), String> {
            panic!("must use snapshot")
        }
        fn service_snapshot(&self, _: &str) -> Result<ServiceSnapshot, String> {
            Ok(self.snapshot.clone())
        }
        fn uses_automatic_proxy(&self, _: &str, _: &ServiceSnapshot) -> Result<bool, String> {
            Ok(self.automatic)
        }
        fn restore_service(&self, _: &str, _: &ServiceSnapshot) -> Result<(), String> {
            panic!("must use unattended writes")
        }
        fn restore_service_unattended(
            &self,
            _: &str,
            target: &ServiceSnapshot,
        ) -> Result<(), String> {
            if self.fail {
                return Err("injected restore failure".into());
            }
            self.writes.lock().unwrap().push(target.clone());
            Ok(())
        }
    }
    fn backend() -> Backend {
        let endpoint = Some(ProxyEndpoint {
            host: "127.0.0.1".into(),
            port: 7898,
        });
        Backend {
            snapshot: ServiceSnapshot {
                web: endpoint.clone(),
                secure: endpoint.clone(),
                socks: endpoint,
                ..Default::default()
            },
            automatic: false,
            fail: false,
            writes: Arc::new(Mutex::new(Vec::new())),
        }
    }
    fn fixture() -> (tempfile::TempDir, Session) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("runtime")).unwrap();
        let journal = serde_json::to_vec(
            &serde_json::json!({"port":7898,"snapshot":{"test":ServiceSnapshot::default()}}),
        )
        .unwrap();
        let descriptor = b"test descriptor";
        std::fs::write(dir.path().join("sysproxy-journal.json"), &journal).unwrap();
        std::fs::write(dir.path().join("runtime/core-descriptor.json"), descriptor).unwrap();
        let session = Session {
            id: "test-session".into(),
            journal_hash: hash(&journal),
            descriptor_hash: hash(descriptor),
            english: false,
        };
        std::fs::write(
            dir.path().join(SESSION),
            serde_json::to_vec(&session).unwrap(),
        )
        .unwrap();
        (dir, session)
    }
    #[test]
    fn recovery_requires_two_failures_and_a_current_session() {
        assert_eq!(
            decide(true, false, &Health::Unhealthy, 1),
            GuardAction::Watch
        );
        assert_eq!(
            decide(true, false, &Health::Unhealthy, 2),
            GuardAction::Restore
        );
        assert_eq!(
            decide(true, false, &Health::Healthy, u32::MAX),
            GuardAction::Watch
        );
        assert_eq!(
            decide(true, false, &Health::Unknown, u32::MAX),
            GuardAction::Watch
        );
        assert_eq!(
            decide(true, true, &Health::Unhealthy, 2),
            GuardAction::Yield
        );
        assert_eq!(
            decide(false, false, &Health::Unhealthy, 2),
            GuardAction::Yield
        );
    }
    #[test]
    fn corrupt_or_legacy_journals_never_authorize_recovery() {
        for raw in [
            r#"{"port":0,"snapshot":{}}"#,
            r#"{"port":65536,"snapshot":{"test":{}}}"#,
            r#"{"port":7898,"enabled":true}"#,
            "invalid",
        ] {
            assert!(valid_journal(raw.as_bytes()).is_none());
        }
    }
    #[test]
    fn cancellation_and_journal_or_core_changes_revoke_authority() {
        for path in [
            SESSION,
            "sysproxy-journal.json",
            "runtime/core-descriptor.json",
        ] {
            let (dir, session) = fixture();
            let b = backend();
            std::fs::write(dir.path().join(path), "replacement").unwrap();
            assert_eq!(
                recover(
                    dir.path(),
                    &session,
                    b.clone(),
                    || Health::Unhealthy,
                    |_| {}
                )
                .unwrap(),
                Outcome::Yield
            );
            assert!(b.writes.lock().unwrap().is_empty());
        }
    }
    #[test]
    fn cancellation_during_health_probe_is_checked_again_before_writing() {
        let (dir, session) = fixture();
        let b = backend();
        let result = recover(
            dir.path(),
            &session,
            b.clone(),
            || {
                std::fs::remove_file(dir.path().join(SESSION)).unwrap();
                Health::Unhealthy
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(result, Outcome::Yield);
        assert!(b.writes.lock().unwrap().is_empty());
    }
    #[test]
    fn restored_foreign_disabled_pac_and_failure_results_are_distinct() {
        for variant in 0..5 {
            let (dir, session) = fixture();
            let mut b = backend();
            if variant == 1 {
                b.snapshot.web = Some(ProxyEndpoint {
                    host: "127.0.0.1".into(),
                    port: 7899,
                });
            }
            if variant == 2 {
                b.snapshot = ServiceSnapshot::default();
            }
            if variant == 3 {
                b.automatic = true;
            }
            if variant == 4 {
                b.fail = true;
            }
            let result = recover(
                dir.path(),
                &session,
                b.clone(),
                || Health::Unhealthy,
                |_| {
                    // Result publication is still inside the app-instance lock.
                    assert!(crate::lock_app_instance(dir.path()).is_err());
                },
            );
            if variant == 4 {
                assert!(result.is_err());
                assert!(dir.path().join("sysproxy-journal.json").exists());
            } else if variant == 0 {
                assert_eq!(result.unwrap(), Outcome::Recovered);
                assert_eq!(b.writes.lock().unwrap().len(), 1);
            } else {
                assert_eq!(result.unwrap(), Outcome::Skipped);
                assert!(b.writes.lock().unwrap().is_empty());
            }
        }
    }
    #[test]
    fn gui_lock_excludes_recovery_after_pid_file_was_removed() {
        let (dir, session) = fixture();
        let b = backend();
        let _gui = crate::lock_app_instance(dir.path()).unwrap();
        assert_eq!(
            recover(
                dir.path(),
                &session,
                b.clone(),
                || Health::Unhealthy,
                |_| {}
            )
            .unwrap(),
            Outcome::Watch
        );
        assert!(b.writes.lock().unwrap().is_empty());
    }
    #[test]
    fn rejected_gui_does_not_revoke_the_guard() {
        let (dir, session) = fixture();
        let _gui = crate::lock_app_instance(dir.path()).unwrap();
        assert!(acquire_gui_instance(dir.path()).is_err());
        assert!(session.current(dir.path()));
    }
    #[test]
    fn gui_start_waits_for_recovery_then_acquires_instance() {
        let (dir, _session) = fixture();
        let recovery = recovery_file(dir.path()).unwrap();
        recovery.try_lock().unwrap();
        let instance = crate::lock_app_instance(dir.path()).unwrap();
        let (sent, received) = std::sync::mpsc::channel();
        let path = dir.path().to_owned();
        let worker = std::thread::spawn(move || {
            let lock = acquire_gui_instance(&path).unwrap();
            sent.send(()).unwrap();
            lock
        });
        assert!(received.recv_timeout(Duration::from_millis(150)).is_err());
        drop(instance);
        drop(recovery);
        received.recv_timeout(Duration::from_secs(3)).unwrap();
        drop(worker.join().unwrap());
    }
    #[test]
    fn live_reused_pid_with_another_binary_is_not_healthy() {
        let (dir, _) = fixture();
        let record = serde_json::json!({"pid":std::process::id(),"port":7898,"secret":"test","binary_path":"/bin/sleep"});
        std::fs::write(
            dir.path().join("runtime/core-descriptor.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        assert_eq!(core_health(dir.path()), Health::Unhealthy);
    }
    #[test]
    fn command_failure_and_timeout_are_reported() {
        assert!(run_command(
            &mut std::process::Command::new("/usr/bin/false"),
            Duration::from_secs(1)
        )
        .is_err());
        assert!(run_command(
            std::process::Command::new("/bin/sleep").arg("2"),
            Duration::from_millis(20)
        )
        .is_err());
    }
    #[test]
    fn plist_escapes_paths_and_binds_data_directory_and_session() {
        let body = plist_body(
            Path::new("/Applications/A&B<C>/Crossbow"),
            Path::new("/tmp/data&test"),
            "session",
            Path::new("/tmp/log"),
        );
        assert!(body.contains("A&amp;B&lt;C&gt;"));
        assert!(body.contains("/tmp/data&amp;test"));
        assert!(body.contains("<string>session</string>"));
    }

    #[test]
    #[ignore = "requires CROSSBOW_CORE_BIN; isolated real-core health only, no system proxy writes"]
    fn native_core_hang_is_unhealthy_even_with_a_listener() {
        let binary = std::fs::canonicalize(
            std::env::var_os("CROSSBOW_CORE_BIN").expect("set CROSSBOW_CORE_BIN"),
        )
        .unwrap();
        let mixed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let controller = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mixed_port = mixed.local_addr().unwrap().port();
        let port = controller.local_addr().unwrap().port();
        let (dir, _) = fixture();
        let config = dir.path().join("test.yaml");
        std::fs::write(&config, format!("mixed-port: {mixed_port}\nexternal-controller: 127.0.0.1:{port}\nsecret: guard-test\nlog-level: error\nrules:\n  - MATCH,DIRECT\n")).unwrap();
        drop(mixed);
        drop(controller);
        let child = std::process::Command::new(&binary)
            .arg("-d")
            .arg(dir.path().join("core"))
            .arg("-f")
            .arg(config)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        struct Child(std::process::Child, bool);
        impl Drop for Child {
            fn drop(&mut self) {
                if !self.1 {
                    return;
                }
                unsafe {
                    libc::kill(self.0.id() as i32, libc::SIGCONT);
                }
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut child = Child(child, true);
        let record = serde_json::json!({"pid":child.0.id(),"port":port,"secret":"guard-test","binary_path":binary});
        std::fs::write(
            dir.path().join("runtime/core-descriptor.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        std::fs::write(dir.path().join("sysproxy-journal.json"),serde_json::to_vec(&serde_json::json!({"port":mixed_port,"snapshot":{"test":ServiceSnapshot::default()}})).unwrap()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while core_health(dir.path()) != Health::Healthy {
            assert!(
                Instant::now() < deadline,
                "isolated core failed to become healthy"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(unsafe { libc::kill(child.0.id() as i32, libc::SIGSTOP) }, 0);
        assert!(crate::lifecycle::exit_flow::port_has_listener(mixed_port));
        assert_eq!(core_health(dir.path()), Health::Unhealthy);
        assert_eq!(unsafe { libc::kill(child.0.id() as i32, libc::SIGCONT) }, 0);
        assert_eq!(core_health(dir.path()), Health::Healthy);
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        child.1 = false;
        assert_eq!(core_health(dir.path()), Health::Unhealthy);
    }
}
