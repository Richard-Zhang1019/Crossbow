//! Private files written by the desktop. Windows uses the user's directory ACL.

use std::fs::{self, File, OpenOptions};
#[cfg(test)]
use std::io::Write as _;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

fn open_private(path: &Path, options: &mut OpenOptions) -> io::Result<File> {
    #[cfg(unix)]
    options
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a regular file",
        ));
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o7777 != 0o600 {
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

pub(crate) fn open_private_append(path: &Path) -> io::Result<File> {
    open_private(path, OpenOptions::new().create(true).append(true))
}

/// Preserve the file handle used for child stdout/stderr.
pub(crate) fn open_private_truncate(path: &Path) -> io::Result<File> {
    let file = open_private(path, OpenOptions::new().create(true).write(true))?;
    file.set_len(0)?;
    Ok(file)
}

/// Replace a snapshot or recovery journal only after writing its full contents.
/// 实现已下沉 crossbow-core::secure_write（#595/#596 统一平台边界），
/// 本封装保留 desktop 调用点与 O_NOFOLLOW 打开家族的桌面差异。
pub(crate) fn write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    crossbow_core::secure_write::write_private_atomic(path, contents)
}

/// Tighten legacy files without following symlinks. Startup treats failure as best effort.
pub(crate) fn ensure_private(path: &Path) {
    #[cfg(unix)]
    let _ = open_private(path, OpenOptions::new().read(true));
    #[cfg(not(unix))]
    let _ = path;
}

/// Call only when no process can be writing the log.
pub(crate) fn trim_private_log(path: &Path) -> io::Result<()> {
    const LIMIT: usize = 256 * 1024;
    let mut file = open_private(path, OpenOptions::new().read(true))?;
    if file.metadata()?.len() <= LIMIT as u64 {
        return Ok(());
    }
    file.seek(SeekFrom::End(-(LIMIT as i64)))?;
    let mut tail = vec![0; LIMIT];
    file.read_exact(&mut tail)?;
    drop(file);
    // The byte limit may land in the middle of a UTF-8 character.
    let partial = tail
        .iter()
        .take(3)
        .take_while(|&&b| b & 0xc0 == 0x80)
        .count();
    write_private(path, &tail[partial..])
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn mode(path: &Path) -> u32 {
        path.metadata().unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn write_private_creates_and_replaces_with_600() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.json");
        write_private(&path, b"{}").unwrap();
        assert_eq!(mode(&path), 0o600);

        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&path, b"[1]").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"[1]");
        assert_eq!(mode(&path), 0o600);
    }

    #[test]
    fn append_tightens_legacy_file_before_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("append.log");
        fs::write(&path, b"old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        let mut file = open_private_append(&path).unwrap();
        assert_eq!(mode(&path), 0o600);
        file.write_all(b" new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"old new");
    }

    #[test]
    fn private_file_operations_leave_symlink_target_alone() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let link = dir.path().join("link");
        fs::write(&target, vec![b'x'; 300 * 1024]).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        ensure_private(&link);
        assert!(open_private_append(&link).is_err());
        assert!(open_private_truncate(&link).is_err());
        assert!(write_private(&link, b"replacement").is_err());
        assert!(trim_private_log(&link).is_err());
        assert_eq!(mode(&target), 0o644);
        assert_eq!(target.metadata().unwrap().len(), 300 * 1024);
    }
}
