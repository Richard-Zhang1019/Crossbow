//! 数据目录文件权限统一（#421）：app 生成的运行时文件一律 0o600。
//!
//! 多用户机器（管理员+日常标准账号，#253 场景）下 644 的 journal/log
//! 会被其他本地账户读取。Windows 用户目录 ACL 默认即私有，unix 才需要
//! 显式 mode。存量文件由 ensure_private 在启动自愈路径收敛。

use std::io::Write;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

/// 以 0o600 创建/追加打开（unix；其他平台等同普通 append）。
pub(crate) fn open_private_append(path: &Path) -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
    }
    #[cfg(not(unix))]
    {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
    }
}

/// 以 0o600 写入（覆盖）。
pub(crate) fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?
            .write_all(contents)
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, contents)
    }
}

/// 启动自愈：把存量文件权限收敛到 0o600（unix）。失败静默——权限收紧
/// 失败不该阻断启动。
pub(crate) fn ensure_private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = path.metadata() {
            let mut perms = meta.permissions();
            if perms.mode() & 0o777 != 0o600 {
                perms.set_mode(0o600);
                let _ = std::fs::set_permissions(path, perms);
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    #[cfg(unix)]
    fn write_private_creates_600_and_rewrites() {
        let dir = std::env::temp_dir().join(format!("cb-perms-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("journal.json");

        write_private(&path, b"{}").unwrap();
        assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);

        // 二次写保留 600 且内容覆盖
        write_private(&path, b"[1]").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"[1]");
        assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);

        // ensure_private 收敛一个 644 的存量文件
        std::fs::write(&path, b"x").unwrap();
        #[allow(unused_mut)]
        let mut perms = path.metadata().unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o644);
            std::fs::set_permissions(&path, perms).unwrap();
            assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o644);
            ensure_private(&path);
            assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        }

        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn open_private_append_keeps_600() {
        let dir = std::env::temp_dir().join(format!("cb-perms-app-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("append.log");
        drop(open_private_append(&path).unwrap());
        assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        drop(open_private_append(&path).unwrap());
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
