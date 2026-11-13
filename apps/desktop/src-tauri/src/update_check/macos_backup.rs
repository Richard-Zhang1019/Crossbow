//! Preserve a verified, independently launchable bundle across Tauri's
//! multi-step replacement. Restore through a sibling staging directory.

use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
use std::path::Path;
use std::process::Command;
use tauri::Manager;

pub(super) fn install(
    app: &tauri::AppHandle,
    replace: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let target = tauri_plugin_updater::extract_path_from_executable(&executable)
        .map_err(|error| error.to_string())?;
    if target
        .extension()
        .is_none_or(|extension| extension != "app")
    {
        return Err("找不到当前 Crossbow 应用包，未开始安装更新".into());
    }
    let backups = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?
        .join("update-backups");
    install_with_backup(&executable, &target, &backups, replace)
}

fn install_with_backup(
    executable: &Path,
    target: &Path,
    backups: &Path,
    replace: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let relative_executable = executable
        .strip_prefix(target)
        .map_err(|error| error.to_string())?;
    let bundle_name = target.file_name().ok_or("无法确定应用包名称")?;
    let old_executable = executable_metadata(executable)?;
    let staging = tempfile::Builder::new()
        .prefix(".staging-")
        .tempdir_in(private_backup_root(backups)?)
        .map_err(|error| format!("无法创建更新备份：{error}"))?;
    let staged_bundle = staging.path().join(bundle_name);
    copy_and_verify(target, &staged_bundle)
        .map_err(|error| format!("无法备份当前应用：{error}"))?;
    executable_metadata(&staged_bundle.join(relative_executable))?;
    let backup_dir = backups.join(uuid::Uuid::new_v4().to_string());
    // Publish only complete backups, outside any auto-cleaned temporary directory.
    fs::rename(staging.path(), &backup_dir)
        .map_err(|error| format!("无法保存旧版应用：{error}"))?;
    let backup = backup_dir.join(bundle_name);

    let outcome = replace()
        .and_then(|()| executable_metadata(executable))
        .and_then(|_| verify_signature(target));
    if let Err(error) = outcome {
        let original_intact = executable_metadata(executable).is_ok_and(|current| {
            current.dev() == old_executable.dev() && current.ino() == old_executable.ino()
        }) && verify_signature(target).is_ok();
        if original_intact {
            let _ = fs::remove_dir_all(&backup_dir);
            return Err(format!("更新未完成：{error}；原应用仍可从原位置打开"));
        }
        let recovery = restore(&backup, target, relative_executable);
        return match recovery {
            Ok(()) => {
                let _ = fs::remove_dir_all(&backup_dir);
                Err(format!("更新未完成：{error}；旧版已恢复到 {}", target.display()))
            }
            Err(recovery_error) => Err(format!(
                "更新未完成：{error}；自动恢复失败：{recovery_error}。请在 Finder 打开 {}，将旧版应用替换回 {}，或直接打开备份中的应用。",
                backup.display(), target.display()
            )),
        };
    }
    if let Err(error) = fs::remove_dir_all(&backup_dir) {
        eprintln!("could not remove completed update backup: {error}");
    }
    Ok(())
}

fn executable_metadata(path: &Path) -> Result<fs::Metadata, String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("无法读取应用可执行文件：{error}"))?;
    if !metadata.is_file() || metadata.mode() & 0o111 == 0 {
        return Err("应用包缺少可执行文件".into());
    }
    Ok(metadata)
}

fn private_backup_root(path: &Path) -> Result<&Path, String> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    match builder.create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("无法创建更新备份目录：{error}")),
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
        return Err("更新备份目录不是私有目录".into());
    }
    Ok(path)
}

fn verify_signature(bundle: &Path) -> Result<(), String> {
    let output = Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(bundle)
        .output()
        .map_err(|error| format!("无法检查应用签名：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "应用签名验证失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

fn copy_and_verify(source: &Path, destination: &Path) -> Result<(), String> {
    // ditto preserves the bundle's symlinks, ACLs and extended attributes.
    let output = Command::new("/usr/bin/ditto")
        .args(["--rsrc", "--extattr", "--acl"])
        .arg(source)
        .arg(destination)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    verify_signature(destination)
}

fn restore(backup: &Path, target: &Path, relative_executable: &Path) -> Result<(), String> {
    let parent = target.parent().ok_or("找不到原安装目录")?;
    let stage = tempfile::Builder::new()
        .prefix(".crossbow-restore-")
        .tempdir_in(parent)
        .map_err(|error| error.to_string())?;
    let staged_bundle = stage
        .path()
        .join(target.file_name().ok_or("找不到应用包名称")?);
    copy_and_verify(backup, &staged_bundle)?;
    executable_metadata(&staged_bundle.join(relative_executable))?;

    let existing = match fs::symlink_metadata(target) {
        Ok(metadata) if metadata.is_dir() => true,
        Ok(_) => return Err("原安装路径已被其他文件占用".into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.to_string()),
    };
    let from = CString::new(staged_bundle.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let to = CString::new(target.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    // Both paths are on the same volume. Swap a partial bundle as a unit,
    // or place the old bundle only if the missing target is still unoccupied.
    let flags = if existing {
        libc::RENAME_SWAP
    } else {
        libc::RENAME_EXCL
    };
    if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), flags) } != 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    executable_metadata(&target.join(relative_executable))?;
    verify_signature(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_missing_and_partial_bundles_as_signed_units() {
        let temp = tempfile::tempdir().unwrap();
        let backup = temp.path().join("backup/Crossbow.app");
        let target = temp.path().join("install/Crossbow.app");
        let binary = Path::new("Contents/MacOS/Crossbow");
        fs::create_dir_all(backup.join(binary.parent().unwrap())).unwrap();
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy("/usr/bin/true", backup.join(binary)).unwrap();
        let plist = concat!(
            r#"<?xml version="1.0" encoding="UTF-8"?>"#,
            r#"<plist version="1.0"><dict>"#,
            "<key>CFBundleIdentifier</key><string>test.crossbow.rollback</string>",
            "<key>CFBundleExecutable</key><string>Crossbow</string>",
            "<key>CFBundlePackageType</key><string>APPL</string>",
            "</dict></plist>"
        );
        fs::write(backup.join("Contents/Info.plist"), plist).unwrap();
        assert!(Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", "-"])
            .arg(&backup)
            .status()
            .unwrap()
            .success());

        restore(&backup, &target, binary).unwrap();
        fs::write(target.join("Contents/partial-new-file"), b"partial").unwrap();
        restore(&backup, &target, binary).unwrap();
        assert!(!target.join("Contents/partial-new-file").exists());
        assert!(Command::new(target.join(binary))
            .status()
            .unwrap()
            .success());
        verify_signature(&backup).unwrap();

        fs::remove_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&backup, &target).unwrap();
        assert!(restore(&backup, &target, binary).is_err());
        assert!(fs::symlink_metadata(&target).unwrap().is_symlink());
        verify_signature(&backup).unwrap();

        fs::remove_file(&target).unwrap();
        restore(&backup, &target, binary).unwrap();
        let backups = temp.path().join("persistent-backups");
        let executable = target.join(binary);
        let error = install_with_backup(&executable, &target, &backups, || {
            Err("authorization cancelled".into())
        })
        .unwrap_err();
        assert!(error.contains("原应用仍可从原位置打开"));
        assert_eq!(fs::read_dir(&backups).unwrap().count(), 0);

        let error = install_with_backup(&executable, &target, &backups, || {
            let published = fs::read_dir(&backups).unwrap().next().unwrap().unwrap();
            verify_signature(&published.path().join("Crossbow.app")).unwrap();
            fs::remove_dir_all(&target).unwrap();
            fs::create_dir_all(&target).unwrap();
            fs::write(target.join("partial"), b"partial").unwrap();
            Err("new bundle move failed".into())
        })
        .unwrap_err();
        assert!(error.contains("旧版已恢复"));
        assert!(!target.join("partial").exists());
        verify_signature(&target).unwrap();
        assert_eq!(fs::read_dir(&backups).unwrap().count(), 0);

        let error = install_with_backup(&executable, &target, &backups, || {
            fs::remove_dir_all(&target).unwrap();
            fs::write(&target, b"unexpected occupant").unwrap();
            Err("new bundle move failed".into())
        })
        .unwrap_err();
        let published = fs::read_dir(&backups).unwrap().next().unwrap().unwrap();
        let preserved = published.path().join("Crossbow.app");
        verify_signature(&preserved).unwrap();
        assert!(error.contains(&preserved.display().to_string()));
        assert_eq!(fs::read(&target).unwrap(), b"unexpected occupant");
    }
}
