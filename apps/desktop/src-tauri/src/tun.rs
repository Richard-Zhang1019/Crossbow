//! TUN 模式特权支撑（macOS）。
//!
//! 思路（对齐 ClashX/Clash Verge）：内核以内置 sing-tun 直接打开系统 utun
//! 接口需要 **root 权限**。不走 LaunchDaemon/SMJobBless 重方案，采用一次性
//! setuid：管理员授权后把内核二进制 chown root + chmod u+s（持久生效，重启
//! 不需要再次授权），应用此后正常拉起内核，子进程自动继承 root 身份。
//!
//! 关闭 TUN 时把 setuid 位摘掉，恢复普通权限。
//! 仅 macOS；Windows 的 TUN 走 wintun + 服务，属 M1.5 后续。

use std::path::PathBuf;

use crate::AppState;

/// 内核二进制的 setuid 状态。
pub fn is_setuid(bin: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(bin)
        .map(|m| m.permissions().mode() & 0o4000 != 0)
        .unwrap_or(false)
}

/// 需要提权时返回内核路径。
fn core_bin(state: &AppState, need: &str) -> Result<PathBuf, String> {
    let bin = crate::core_download::resolve_core_for(&state.data_dir, Some(need))
        .ok_or_else(|| format!("内核 {need} 未安装"))?;
    Ok(bin)
}

/// 确保 TUN 目标内核是 setuid root。首次会弹管理员授权（osascript
/// with administrator privileges），成功后持久生效。
pub fn ensure_root_binary(state: &AppState) -> Result<(), String> {
    let is_singbox = {
        let store = state.store.lock().unwrap();
        store.data().engine.engine == crossbow_core::Engine::SingBox
    };
    let need = if is_singbox { "sing-box" } else { "mihomo" };
    let bin = core_bin(state, need)?;
    if is_setuid(&bin) {
        return Ok(());
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
    Ok(())
}

/// 关闭 TUN：摘掉 setuid 位（尽力而为，失败不打断流程）。
pub fn clear_root_binary(state: &AppState) {
    let is_singbox = {
        let store = state.store.lock().unwrap();
        store.data().engine.engine == crossbow_core::Engine::SingBox
    };
    let need = if is_singbox { "sing-box" } else { "mihomo" };
    let Ok(bin) = core_bin(state, need) else { return };
    let _ = std::process::Command::new("osascript")
        .args([
            "-e",
            &format!(
                "do shell script \"chmod u-s '{}'\" with administrator privileges",
                bin.display()
            ),
        ])
        .output();
}

/// TUN 与系统代理互斥：TUN 接管全局路由后系统代理冗余且有环路风险。
pub fn toggle_tun(state: &AppState, enable: bool) -> Result<(), String> {
    {
        let mut store = state.store.lock().unwrap();
        store.data_mut().engine.tun_enable = enable;
        store.save().map_err(|e| e.to_string())?;
    }
    if enable {
        // 先关系统代理（不发事件，最终状态在重启内核后统一刷新）
        state.sysproxy.disable();
    } else {
        // 摘掉 setuid 位，恢复普通权限
        clear_root_binary(state);
    }
    // 内核运行中则热重启应用 TUN 配置
    if crate::CoreStatus::Running == state.core().status() {
        crate::restart_core(state)?;
    }
    Ok(())
}
