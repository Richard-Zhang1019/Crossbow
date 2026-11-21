//! 跨平台私有安全写入边界（#595/#596 工作包 C1）。
//!
//! Store 配置与快照、desktop 的 journal/log 写入共用这一份实现——避免
//! core 与 desktop 各自维护不一致的平台语义。保证：
//!
//! - **私有**：Unix 普通文件 0600（显式 mode + 旧文件收敛），专用目录
//!   0700；Windows 依赖用户目录 ACL 继承（`C:\Users\<u>\AppData` 天然
//!   仅本用户 + SYSTEM + 管理员可见），POSIX mode 无效也不需要。
//! - **原子**：同目录随机名独占创建临时文件（`create_new` 天然拒绝
//!   攻击者预置的 symlink 占位——已存在即失败，不存在 O_NOFOLLOW 需求）
//!   → 全量写入 → 文件 `sync_all`（Windows 为 FlushFileBuffers）→
//!   rename 原子替换 → 失败清理本次临时文件。
//! - **耐久**：rename 后 Unix 尽力同步父目录（Linux 生效；macOS 对目录
//!   fd 的 fsync 返回 EINVAL，忽略——APFS 日志承担目录项持久化；Windows
//!   无目录 fsync API，依赖 NTFS 元数据日志）。
//! - **边界**：只处理 Crossbow 拥有的路径；目标为符号链接/目录时明确
//!   拒绝，不跟随、不覆盖；不递归触碰用户其他文件；失败不写日志内容。

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
    use std::cell::Cell;

    thread_local! {
        static FAIL_AT: Cell<usize> = const { Cell::new(0) };
        static STEP: Cell<usize> = const { Cell::new(0) };
    }

    pub(crate) fn step_gate(step: super::IoStep) -> Result<(), std::io::Error> {
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

    /// 持注入的 guard：离开任何退出路径时复位。
    pub struct InjectGuard;

    impl Drop for InjectGuard {
        fn drop(&mut self) {
            FAIL_AT.with(|f| f.set(0));
            reset_steps();
        }
    }

    /// 注入第 `n` 个 IO 步骤失败（本线程），guard 释放时复位。
    pub fn inject_at(n: usize) -> InjectGuard {
        FAIL_AT.with(|f| f.set(n));
        reset_steps();
        InjectGuard
    }
}

#[cfg(any(test, feature = "test-io-inject"))]
pub use inject::inject_at;
#[cfg(any(test, feature = "test-io-inject"))]
use inject::step_gate;

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
    inject::reset_steps();
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
    // 显式 mode 0600：任意 umask 下屏蔽位只可能去掉 group/other 权限，
    // 0600 本就没有这些位——私有语义不变。
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
        step_gate(IoStep::Write)?;
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
    // 目录项持久化：Linux fsync 目录生效；macOS/Windows 平台语义见模块注释
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        if let Ok(dir) = File::open(parent) {
            let _ = step_gate(IoStep::ParentSync).and_then(|()| dir.sync_all());
        }
    }
    Ok(())
}

/// 收敛既有文件权限到私有（Unix 0600；Windows 依赖目录 ACL，no-op）。
/// 不跟随符号链接；失败为 best effort（调用方自决是否上报）。
pub fn ensure_private(path: &Path) -> io::Result<()> {
    let meta = path.symlink_metadata()?;
    if !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a regular file",
        ));
    }
    #[cfg(unix)]
    {
        let mode = meta.permissions().mode() & 0o7777;
        if mode != 0o600 {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// 专用目录权限收敛（Unix 0700；Windows no-op）。
pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        let meta = dir.symlink_metadata()?;
        let mode = meta.permissions().mode() & 0o7777;
        if mode != 0o700 {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn mode(path: &Path) -> u32 {
        path.metadata().unwrap().permissions().mode() & 0o777
    }

    /// umask 022/002/077 下 0600 的保证来自显式 mode——屏蔽位只可能去掉
    /// group/other 权限，而 0600 没有这些权限，任意 umask 下不变。
    #[test]
    fn private_write_is_0600_under_any_umask() {
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

        // 步骤序列：1 create / 2 write / 3 sync / 4 rename。parent-sync
        // 不在注入矩阵：其失败在生产语义下是容忍的（macOS 目录 fsync
        // 恒 EINVAL），仅目录项持久性降级，不产生数据损坏。
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
