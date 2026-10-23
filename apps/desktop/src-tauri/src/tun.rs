//! TUN 模式特权支撑（macOS）。
//!
//! 思路（对齐 ClashX/Clash Verge）：内核以内置 sing-tun 直接打开系统 utun
//! 接口需要 **root 权限**。不走 LaunchDaemon/SMJobBless 重方案，采用一次性
//! setuid：管理员授权后把内核二进制 chown root + chmod u+s（持久生效，重启
//! 不需要再次授权），应用此后正常拉起内核，子进程自动继承 root 身份。
//!
//! 授权后的版本化副本保留 setuid，关闭 TUN 时不重复请求管理员授权。
//! 仅 macOS；Windows 的 TUN 走 wintun + 服务，属 M1.5 后续。

use std::path::{Path, PathBuf};

use crate::AppState;

/// 内核二进制的 setuid 状态。
pub fn is_setuid(bin: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(bin)
        .map(|m| m.permissions().mode() & 0o4000 != 0)
        .unwrap_or(false)
}

/// 把 TUN 内核放到版本化的用户数据目录，避免更新 `.app` 时覆盖掉
/// 已授权的 setuid 文件。只有核心版本变化时才需要重新授权。
fn persistent_core_bin(state: &AppState, source: &Path) -> Result<PathBuf, String> {
    let output = std::process::Command::new(source)
        .arg(if crate::core_download::is_core_bin(source, "sing-box") {
            "version"
        } else {
            "-v"
        })
        .output()
        .map_err(|e| format!("读取内核版本失败：{e}"))?;
    if !output.status.success() {
        return Err("读取内核版本失败".into());
    }
    let version = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>();
    if version.is_empty() {
        return Err("内核未返回版本信息".into());
    }
    let name = source
        .file_name()
        .ok_or_else(|| "内核路径缺少文件名".to_string())?;
    let target = state.data_dir.join("tun-binaries").join(version).join(name);
    if !target.exists() {
        let parent = target
            .parent()
            .ok_or_else(|| "TUN 内核目标路径无效".to_string())?;
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        std::fs::copy(source, &target).map_err(|e| format!("复制 TUN 内核失败：{e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| format!("设置 TUN 内核权限失败：{e}"))?;
        }
    }
    Ok(target)
}

/// 确保 TUN 目标内核是 setuid root，并返回持久化副本路径。
pub fn ensure_root_binary(state: &AppState, source: &Path) -> Result<PathBuf, String> {
    let bin = persistent_core_bin(state, source)?;
    if is_setuid(&bin) {
        return Ok(bin);
    }
    let script = format!(
        "chown root '{}'; chmod u+s '{}'",
        bin.display(),
        bin.display()
    );
    let out = std::process::Command::new("osascript")
        .args([
            "-e",
            &format!(
                "do shell script \"{}\" with administrator privileges",
                script.replace('"', "\\\"")
            ),
        ])
        .output()
        .map_err(|e| format!("run osascript: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "管理员授权失败：{}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    if !is_setuid(&bin) {
        return Err("提权后 setuid 位未生效".into());
    }
    Ok(bin)
}

/// 是否存在已授权（setuid root）的 TUN 内核副本。PR13 后授权副本在
/// tun-binaries/<版本>/ 下，与解析出的普通内核二进制分离——状态展示
/// 检查这里，而不是普通内核的 setuid 位（否则永远 false）。
pub fn authorized_copy_exists(data_dir: &std::path::Path, need: &str) -> bool {
    let bin_name = crate::core_download::core_bin_file(need);
    let Ok(versions) = std::fs::read_dir(data_dir.join("tun-binaries")) else {
        return false;
    };
    for version in versions.flatten() {
        let copy = version.path().join(&bin_name);
        if let Ok(meta) = std::fs::metadata(&copy) {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o4000 != 0 {
                return true;
            }
        }
    }
    false
}

/// TUN 与系统代理互斥：TUN 接管全局路由后系统代理冗余且有环路风险。
pub fn toggle_tun(state: &AppState, enable: bool) -> Result<(), String> {
    let previous_tun = {
        let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store.data().engine.tun_enable
    };
    if previous_tun == enable {
        return Ok(());
    }
    let was_running = crate::CoreStatus::Running == state.core().status();
    let previous_proxy = state.sysproxy.status();
    {
        let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store.data_mut().engine.tun_enable = enable;
        if let Err(error) = store.save() {
            store.data_mut().engine.tun_enable = previous_tun;
            return Err(error.to_string());
        }
    }
    if enable {
        // 先关系统代理（不发事件，最终状态在重启内核后统一刷新）
        state.sysproxy.disable();
    }
    // 开启 TUN 时即使核心原本停止，也必须启动核心，否则 UI 会显示 TUN 已开，
    // 但实际没有 utun、DNS 或日志；核心已运行时则重启以应用新配置。
    let should_start = was_running || enable;
    if should_start {
        let start_result = if was_running {
            crate::restart_core(state)
        } else {
            crate::start_core(state)
        };
        if let Err(start_error) = start_result {
            // A failed TUN restart (most commonly a cancelled macOS admin
            // prompt) must not leave the saved toggle enabled while the core
            // is stopped. Restore the previous setting and bring the previous
            // non-TUN core/proxy state back whenever possible.
            let rollback_error = {
                let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
                store.data_mut().engine.tun_enable = previous_tun;
                store.save().err().map(|e| e.to_string())
            };
            let recovery_error = (was_running || previous_proxy.enabled)
                .then(|| crate::start_core(state).err())
                .flatten();
            if recovery_error.is_none() && previous_proxy.enabled && !previous_tun {
                if let Err(error) = state.sysproxy.enable(previous_proxy.port) {
                    return Err(format!(
                        "{start_error}; TUN 设置已回滚，但恢复系统代理失败：{error}"
                    ));
                }
            }
            let details = [rollback_error, recovery_error]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            if details.is_empty() {
                return Err(format!("TUN 切换失败，已恢复之前的运行状态：{start_error}"));
            }
            return Err(format!(
                "TUN 切换失败：{start_error}；恢复旧状态时遇到问题：{}",
                details.join("；")
            ));
        }
    }
    Ok(())
}
