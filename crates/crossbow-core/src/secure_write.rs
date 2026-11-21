//! 跨平台私有安全写入边界（#595/#596 工作包 C1）。
//!
//! Store 配置与快照、desktop 的 journal/log 写入共用这一份实现——避免
//! core 与 desktop 各自维护不一致的平台语义。保证：
//!
//! - Unix files are 0600 and owned directories 0700. Windows inherits the
//!   parent directory ACL; this is not an explicit ACL migration guarantee.
//! - An exclusively created temporary file is written and synced before rename.
//!   Failures before rename keep the old target; failures after rename may leave
//!   the complete new target installed and must still be reported to the caller.
//! - Unix parent directory sync errors are reported, except when the filesystem
//!   explicitly reports that directory sync is unsupported. Windows uses the
//!   standard rename implementation without claiming power-loss guarantees.
//! - Permission migration uses no-follow handles and only changes owned paths.
//!   Tests inject failures; they do not simulate actual power loss.

#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// 故障注入步骤（#596 验收：注入创建、写入、sync、替换各阶段失败）。
/// 生产路径 `step_gate` 恒 Ok，零开销。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IoStep {
    Create,
    Write,
    Sync,
    Rename,
    #[cfg(unix)]
    ParentSync,
    Cleanup,
}

/// 注入状态线程本地化（#596 故障注入）：注入只影响本测试线程的 IO
/// 调用——cargo test 并行线程互不污染，无需全局锁。guard 离开任何
/// 退出路径（含断言 panic）时复位。
#[cfg(any(test, feature = "test-io-inject"))]
mod inject {
    use std::cell::{Cell, RefCell};
    use std::path::{Path, PathBuf};

    thread_local! {
        static FAIL_AT: Cell<usize> = const { Cell::new(0) };
        static STEP: Cell<usize> = const { Cell::new(0) };
        static TARGET: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
        static ACTIVE: Cell<bool> = const { Cell::new(true) };
    }

    pub(crate) fn step_gate(step: super::IoStep) -> Result<(), std::io::Error> {
        if !ACTIVE.with(Cell::get) {
            return Ok(());
        }
        let n = STEP.with(|s| {
            let next = s.get() + 1;
            s.set(next);
            next
        });
        if FAIL_AT.with(|f| f.get()) == n {
            return Err(std::io::Error::other(format!(
                "injected failure at {step:?}"
            )));
        }
        Ok(())
    }

    pub(crate) fn reset_steps() {
        STEP.with(|s| s.set(0));
    }

    pub(crate) fn begin_write(path: &Path) {
        reset_steps();
        ACTIVE.with(|active| {
            active.set(TARGET.with(|target| target.borrow().as_ref().is_none_or(|p| p == path)));
        });
    }

    /// 持注入的 guard：离开任何退出路径时复位。
    pub struct InjectGuard;

    impl Drop for InjectGuard {
        fn drop(&mut self) {
            FAIL_AT.with(|f| f.set(0));
            TARGET.with(|target| *target.borrow_mut() = None);
            ACTIVE.with(|active| active.set(true));
            reset_steps();
        }
    }

    /// 注入第 `n` 个 IO 步骤失败（本线程），guard 释放时复位。
    pub fn inject_at(n: usize) -> InjectGuard {
        FAIL_AT.with(|f| f.set(n));
        TARGET.with(|target| *target.borrow_mut() = None);
        ACTIVE.with(|active| active.set(true));
        reset_steps();
        InjectGuard
    }

    /// Target a specific file so a successful snapshot does not consume the fixture.
    pub fn inject_path_at(path: &Path, n: usize) -> InjectGuard {
        let guard = inject_at(n);
        TARGET.with(|target| *target.borrow_mut() = Some(path.to_owned()));
        guard
    }
}

#[cfg(any(test, feature = "test-io-inject"))]
use inject::step_gate;
#[cfg(any(test, feature = "test-io-inject"))]
pub use inject::{inject_at, inject_path_at};

#[cfg(not(any(test, feature = "test-io-inject")))]
fn step_gate(_step: IoStep) -> io::Result<()> {
    Ok(())
}

fn unique_temp_path(target: &Path) -> PathBuf {
    use std::sync::atomic::AtomicU64;
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::SeqCst);
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    target.with_file_name(format!(
        ".{name}.{}.{}.{}.tmp",
        std::process::id(),
        nanos,
        seq
    ))
}

/// 私有安全替换写入：旧文件在 rename 成功前保持原样（写入/同步失败的
/// 任何阶段都不破坏最后可用版本）。
pub fn write_private_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    #[cfg(any(test, feature = "test-io-inject"))]
    inject::begin_write(path);
    // 目标边界：符号链接与目录明确拒绝（不跟随、不覆盖）
    match path.symlink_metadata() {
        Ok(meta) if !meta.is_file() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected a regular file",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let temp = unique_temp_path(path);
    // 独占创建：目标名被占位（含 symlink）即失败
    step_gate(IoStep::Create)?;
    // Explicit private mode; supported umasks 022/002/077 preserve owner access.
    let mut file = {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&temp)?
        }
        #[cfg(not(unix))]
        {
            OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temp)?
        }
    };
    let result = (|| -> io::Result<()> {
        let write_check = step_gate(IoStep::Write);
        // A write failure fixture leaves a partial temporary file to verify cleanup.
        #[cfg(any(test, feature = "test-io-inject"))]
        if write_check.is_err() {
            file.write_all(&contents[..contents.len() / 2])?;
        }
        write_check?;
        file.write_all(contents)?;
        step_gate(IoStep::Sync)?;
        file.sync_all()
    })();
    drop(file);
    let result = match result {
        // rename 注入失败也不能走 `?` 提前返回——必须落入清理块移除本次
        // 临时文件（#596：无残留）。
        Ok(()) => step_gate(IoStep::Rename).and_then(|()| fs::rename(&temp, path)),
        Err(error) => Err(error),
    };
    if result.is_err() {
        step_gate(IoStep::Cleanup)?;
        let _ = fs::remove_file(&temp);
        return result;
    }
    #[cfg(unix)]
    {
        step_gate(IoStep::ParentSync)?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let dir = File::open(parent)?;
        if let Err(error) = dir.sync_all() {
            // EINVAL/Unsupported are filesystem capabilities, not a blanket
            // macOS exemption. EIO, permission and open errors must propagate.
            if error.raw_os_error() != Some(libc::EINVAL)
                && error.kind() != io::ErrorKind::Unsupported
            {
                return Err(error);
            }
        }
    }
    Ok(())
}

/// Tighten an existing regular file without following symlinks.
pub fn ensure_private(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected a regular file",
            ));
        }
        if meta.permissions().mode() & 0o7777 != 0o600 {
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
    }
    #[cfg(not(unix))]
    if !path.symlink_metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a regular file",
        ));
    }
    Ok(())
}

/// Prepare an owned directory; never chmod a symlink's target or existing parents.
pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    match dir.symlink_metadata() {
        Ok(meta) if !meta.is_dir() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected a directory",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(dir)?;
        }
        Err(error) => return Err(error),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let handle = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(dir)?;
        let meta = handle.metadata()?;
        if meta.permissions().mode() & 0o7777 != 0o700 {
            handle.set_permissions(fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn mode(path: &Path) -> u32 {
        path.metadata().unwrap().permissions().mode() & 0o777
    }

    /// Change the process-wide umask only in isolated test subprocesses.
    #[test]
    fn private_write_is_0600_under_supported_umasks() {
        if let Ok(mask) = std::env::var("CROSSBOW_SECURE_WRITE_TEST_UMASK") {
            let mask = u32::from_str_radix(&mask, 8).unwrap();
            unsafe {
                libc::umask(mask as libc::mode_t);
            }
        } else {
            for mask in ["022", "002", "077"] {
                let result = std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "secure_write::tests::private_write_is_0600_under_supported_umasks",
                    ])
                    .env("CROSSBOW_SECURE_WRITE_TEST_UMASK", mask)
                    .output()
                    .unwrap();
                assert!(result.status.success(), "mask {mask}: {:?}", result);
            }
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.json");
        write_private_atomic(&path, br#"{"v":1}"#).unwrap();
        assert_eq!(mode(&path), 0o600);
        assert!(fs::read_dir(dir.path()).unwrap().all(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with("store.json")));
        // 无临时文件残留
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn replace_tightens_loose_legacy_target() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.json");
        fs::write(&path, b"legacy").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        write_private_atomic(&path, b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(mode(&path), 0o600);
    }

    #[test]
    fn rejects_symlink_and_directory_targets() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.json");
        fs::write(&target, b"keep").unwrap();
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(write_private_atomic(&link, b"overwrite").is_err());
        assert_eq!(fs::read(&target).unwrap(), b"keep");
        assert!(write_private_atomic(dir.path(), b"dir").is_err());
    }

    /// #596 验收：注入各阶段失败——旧文件或新文件至少一个完整可解析，
    /// 临时文件不残留。
    #[test]
    fn injected_failures_never_corrupt_last_good_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.json");
        fs::write(&path, br#"{"good":true}"#).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        // 1 create / 2 partial write / 3 file sync / 4 rename.
        // Post-rename directory sync has its own complete-new-file regression.
        for fail_at in 1..=4 {
            let _inject = inject_at(fail_at);
            let result = write_private_atomic(&path, br#"{"next":2}"#);
            assert!(result.is_err(), "step {fail_at} must fail");
            // rename 前失败：旧文件完整可解析、未被破坏
            assert_eq!(fs::read(&path).unwrap(), br#"{"good":true}"#);
            assert_eq!(mode(&path), 0o600);
            // 临时文件不残留
            let leftovers: Vec<_> = fs::read_dir(dir.path())
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
                .collect();
            assert!(leftovers.is_empty(), "step {fail_at} left tmp behind");
        }
        let result = write_private_atomic(&path, br#"{"next":2}"#);
        assert!(
            result.is_ok(),
            "clean write after guard drop must succeed: {result:?}"
        );
        assert_eq!(fs::read(&path).unwrap(), br#"{"next":2}"#);
    }

    #[test]
    fn private_directory_rejects_symlinks_without_chmod_target() {
        let root = tempfile::tempdir().unwrap();
        let outside = root.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o755)).unwrap();
        let link = root.path().join("linked");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert!(ensure_private_dir(&link).is_err());
        assert_eq!(mode(&outside), 0o755);
    }

    #[test]
    fn parent_sync_failure_reports_error_after_complete_replacement() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("store.json");
        fs::write(&path, br#"{"old":true}"#).unwrap();
        let _inject = inject_at(5);
        assert!(write_private_atomic(&path, br#"{"new":true}"#).is_err());
        assert_eq!(fs::read(&path).unwrap(), br#"{"new":true}"#);
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn ensure_private_rejects_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("t");
        fs::write(&target, b"x").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        let link = dir.path().join("l");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(ensure_private(&link).is_err());
        // 符号链接目标未被触碰（不跟随）
        assert_eq!(mode(&target), 0o644);
        assert_eq!(fs::read(&target).unwrap(), b"x");
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    /// Windows 平台替换语义（#596 验收「Unix 与 Windows 替换测试分别
    /// 记录」）：std::fs::rename 在 Windows 带 REPLACE_EXISTING 语义，
    /// 覆盖既有目标成功；权限由用户目录 ACL 继承保证（无 POSIX mode）。
    /// CI windows job 是本测试的验证环境。
    #[test]
    fn windows_replace_overwrites_existing_target() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.json");
        fs::write(&path, b"old").unwrap();
        write_private_atomic(&path, b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        // 临时文件不残留
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }
}
