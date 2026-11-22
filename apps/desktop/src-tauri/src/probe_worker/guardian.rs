//! Independent lifetime guardian: GUI pipe EOF or 30-second deadline reaps the owned core.
//! Runs before Tauri/single-instance/network recovery initialization.
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
pub const ARG: &str = "--crossbow-node-probe-worker";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Launch {
    pub version: u8,
    pub binary: PathBuf,
    pub mihomo: bool,
}
fn validated(dir: &Path) -> Result<PathBuf, String> {
    let meta = dir.symlink_metadata().map_err(|_| "missing directory")?;
    if !meta.is_dir() {
        return Err("not a private directory".into());
    }
    let canonical = dir.canonicalize().map_err(|_| "invalid directory")?;
    let root = std::env::temp_dir()
        .canonicalize()
        .map_err(|_| "invalid temporary root")?;
    if canonical.parent() != Some(root.as_path())
        || !canonical
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("crossbow-probe-"))
    {
        return Err("directory outside probe root".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // SAFETY: reading the current effective uid has no preconditions.
        if meta.permissions().mode() & 0o777 != 0o700 || meta.uid() != unsafe { libc::geteuid() } {
            return Err("directory is not private".into());
        }
    }
    Ok(canonical)
}
fn run(dir: &Path) -> Result<(), String> {
    let dir = validated(dir)?;
    let marker = dir.join("launch.json");
    let meta = marker
        .symlink_metadata()
        .map_err(|_| "missing launch marker")?;
    if !meta.is_file() || meta.len() > 8192 {
        return Err("invalid launch marker".into());
    }
    let launch: Launch =
        serde_json::from_slice(&std::fs::read(&marker).map_err(|_| "cannot read marker")?)
            .map_err(|_| "invalid marker")?;
    if launch.version != 1 || !launch.binary.is_absolute() {
        return Err("invalid launch descriptor".into());
    }
    #[cfg(windows)]
    let _job = job::OwnJob::attach()?;
    let path = dir.join("probe.json");
    let mut command = crate::process::command(launch.binary);
    if launch.mihomo {
        command.arg("-d").arg(&dir).arg("-f").arg(path);
    } else {
        command.arg("run").arg("-D").arg(&dir).arg("-c").arg(path);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn().map_err(|_| "cannot spawn owned core")?;
    let disconnected = Arc::new(AtomicBool::new(false));
    let signal = disconnected.clone();
    let reader = std::thread::Builder::new()
        .name("probe-parent-pipe".into())
        .spawn(move || {
            let mut input = [0; 1];
            let _ = std::io::stdin().read(&mut input);
            signal.store(true, Ordering::SeqCst);
        });
    if reader.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);
        return Err("cannot watch parent".into());
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    while !disconnected.load(Ordering::SeqCst) && Instant::now() < deadline {
        match child.try_wait() {
            Ok(None) => {}
            _ => break,
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
    // Only a verified, private, one-level probe folder is removed.
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
pub fn maybe_run() -> bool {
    if std::env::args_os().nth(1).as_deref() != Some(std::ffi::OsStr::new(ARG)) {
        return false;
    }
    let result = std::env::args_os()
        .nth(2)
        .ok_or("missing probe directory".to_owned())
        .and_then(|p| run(Path::new(&p)));
    std::process::exit(if result.is_ok() { 0 } else { 70 });
}
#[cfg(windows)]
mod job {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::{
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
                SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            },
            Threading::GetCurrentProcess,
        },
    };
    pub struct OwnJob(HANDLE);
    impl OwnJob {
        pub fn attach() -> Result<Self, String> {
            // SAFETY: valid local buffers and owned handles; the guardian joins its own job before spawning children.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return Err("cannot create worker job".into());
                }
                let guard = Self(handle);
                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as *const _,
                    std::mem::size_of_val(&limits) as u32,
                ) == 0
                    || AssignProcessToJobObject(handle, GetCurrentProcess()) == 0
                {
                    return Err("cannot contain worker processes".into());
                }
                Ok(guard)
            }
        }
    }
    impl Drop for OwnJob {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guardian_refuses_arbitrary_or_shared_directories() {
        let dir = tempfile::tempdir().unwrap();
        assert!(validated(dir.path()).is_err());
        let dir = tempfile::Builder::new()
            .prefix("crossbow-probe-")
            .tempdir()
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        assert!(validated(dir.path()).is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(validated(dir.path()).is_err());
        }
    }
}
