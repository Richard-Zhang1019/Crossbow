//! TUN 模式特权支撑（macOS）。
//!
//! 思路（对齐 ClashX/Clash Verge）：内核以内置 sing-tun 直接打开系统 utun
//! 接口需要 **root 权限**。不走 LaunchDaemon/SMJobBless 重方案，采用一次性
//! setuid：管理员授权后把内核二进制 chown root + chmod u+s（持久生效，重启
//! 不需要再次授权），应用此后正常拉起内核，子进程自动继承 root 身份。
//!
//! 授权后的版本化副本保留 setuid，关闭 TUN 时不重复请求管理员授权。
//! 仅 macOS；Windows 的 TUN 走 wintun + 服务，属 M1.5 后续。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::AppState;

/// 内核二进制的 setuid 状态。
pub fn is_setuid(bin: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(bin)
        .map(|m| m.permissions().mode() & 0o4000 != 0)
        .unwrap_or(false)
}

/// 物理网络是否有原生（非隧道）IPv6 默认路由。TUN 的 inet6 地址/分流
/// 路由注入与 dns.ipv6 仅在原生 v6 可达时才有意义：v4-only 网络上
/// DIRECT 的 v6 目的地从物理网卡发不出去（#136 实测 no route to host）。
/// netstat 失败按「有 v6」处理（fail-open 保持现行为）。
pub fn has_native_ipv6() -> bool {
    let Ok(output) = std::process::Command::new("netstat")
        .args(["-rn", "-f", "inet6"])
        .output()
    else {
        return true;
    };
    native_v6_default(&String::from_utf8_lossy(&output.stdout))
}

/// netstat -rn -f inet6 行解析：存在 default 路由且出口接口不是 utun
/// 即为原生 v6（fe80 网关经物理口也算——路由器通告的原生 v6 常见形态）。
/// 列结构复用 coexist 的按空白切列模式：Dest Gateway Flags Netif [Expire]。
fn native_v6_default(text: &str) -> bool {
    text.lines().any(|line| {
        let cols: Vec<&str> = line.split_whitespace().collect();
        match cols.as_slice() {
            [dest, _gateway, _flags, netif, ..] => *dest == "default" && !netif.starts_with("utun"),
            _ => false,
        }
    })
}

/// TUN 接管漂移检测（#154）：捕获路由在位是唯一不变量。系统 DNS 停在
/// 路由器/公共 DNS 是健康稳态——dns-hijack any:53 会在捕获路由把 :53
/// 查询带进 TUN 时劫持返回 fake-ip，DNS 指向哪与接管无关（把 DNS 纳入
/// 检查曾造成永久误判漂移、TUN 每 15 秒重建立一次，#163）。
/// netstat 失败按「在位」处理（fail-open 保持现行为）。
#[cfg(target_os = "macos")]
pub fn takeover_drift() -> bool {
    !capture_route_present()
}

/// v4 捕获路由在位：default 或分片默认路由经 TUN 接口
#[cfg(target_os = "macos")]
fn capture_route_present() -> bool {
    let Ok(output) = std::process::Command::new("netstat")
        .args(["-rn", "-f", "inet"])
        .output()
    else {
        return true;
    };
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().any(|line| {
        let cols: Vec<&str> = line.split_whitespace().collect();
        match cols.as_slice() {
            [dest, _gateway, _flags, netif, ..] => {
                is_default_route(dest) && netif.starts_with("utun")
            }
            _ => false,
        }
    })
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

/// Remove authorized kernel copies for older versions once the current copy is
/// ready. Version directories are created by this app under data_dir, and the
/// current version is retained so a failed/partial upgrade cannot delete the
/// binary that is about to be launched.
fn cleanup_stale_tun_binaries(data_dir: &Path, current_bin: &Path) {
    let binaries_dir = data_dir.join("tun-binaries");
    let current_version_dir = current_bin.parent();
    let Ok(entries) = std::fs::read_dir(&binaries_dir) else {
        return;
    };

    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let version_dir = entry.path();
        if current_version_dir == Some(version_dir.as_path())
            || !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false)
        {
            continue;
        }
        if let Err(error) = std::fs::remove_dir_all(&version_dir) {
            eprintln!(
                "tun: failed to remove stale authorized kernel copy {}: {error}",
                version_dir.display()
            );
        }
    }
}

/// 确保 TUN 目标内核是 setuid root，并返回持久化副本路径。
pub fn ensure_root_binary(state: &AppState, source: &Path) -> Result<PathBuf, String> {
    let bin = persistent_core_bin(state, source)?;
    if !is_setuid(&bin) {
        crate::authorization::require_interactive()?;
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
    }
    cleanup_stale_tun_binaries(&state.data_dir, &bin);
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

/// Mihomo's macOS auto-route installs the 1.0.0.0/8 route. If another tunnel
/// already owns it, mihomo can report a healthy controller while its TUN
/// listener failed. Catch this before changing the saved switch or proxy.
fn ipv4_routes() -> Result<Vec<(String, String)>, String> {
    let output = std::process::Command::new("netstat")
        .args(["-rn", "-f", "inet"])
        .output()
        .map_err(|e| format!("检查 TUN 路由失败：{e}"))?;
    if !output.status.success() {
        return Err("检查 TUN 路由失败".into());
    }
    let table = String::from_utf8_lossy(&output.stdout);
    let mut routes = Vec::new();
    for line in table.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if let (Some(destination), Some(interface)) = (fields.first(), fields.get(3)) {
            routes.push(((*destination).to_string(), (*interface).to_string()));
        }
    }
    Ok(routes)
}

fn utun_routes() -> Result<BTreeSet<(String, String)>, String> {
    Ok(ipv4_routes()?
        .into_iter()
        .filter(|(_, interface)| interface.starts_with("utun"))
        .collect())
}

/// macOS renders a full-default route as 1, 2/7, 4/6, ..., 128.0/1 instead of
/// a literal 0.0.0.0/0. Treat any utun-owned prefix in that set as a tunnel.
fn is_default_route(destination: &str) -> bool {
    matches!(destination, "0" | "0/0" | "0.0.0.0/0" | "default")
        || matches!(destination, "1" | "1/8" | "1.0.0.0/8")
        || matches!(
            destination,
            "2/7" | "4/6" | "8/5" | "16/4" | "32/3" | "64/2" | "128.0/1"
        )
}

fn ensure_route_available() -> Result<BTreeSet<(String, String)>, String> {
    let routes = ipv4_routes()?;
    for (destination, interface) in &routes {
        // 只有 utun 接口上的默认路由才是"其他隧道"——en0 等物理网卡的
        // 默认网关是每台 Mac 的常态，不能当作冲突（否则开 TUN 必失败）。
        if is_default_route(destination) && interface.starts_with("utun") {
            return Err(format!(
                "已有其他隧道占用默认路由（{interface}）。请关闭其他代理或 VPN 的 TUN 模式后重试"
            ));
        }
    }
    Ok(routes
        .into_iter()
        .filter(|(_, interface)| interface.starts_with("utun"))
        .collect())
}

#[cfg(test)]
mod route_tests {
    /// #136：netstat -f inet6 解析——fe80/utun 噪音不算原生 v6；
    /// 经物理口的 default（fe80 网关或全局网关）算；空表不算；
    /// 残行（列数不足）不 panic 也不误判。
    /// #154：接管漂移解析——分片/完整 default 经 utun 即捕获在位；
    /// 只有 fe80/物理口路由 = 漂移；scutil 输出含 198.18.0.x = DNS 接管。
    #[test]
    fn capture_route_detection() {
        let captured = concat!(
            "Routing tables\n",
            "default 192.168.31.1 UGScg en0\n",
            "0/1 198.18.0.1 UGScg utun4\n",
            "128.0/1 198.18.0.1 UGScg utun4\n",
        );
        let lost = "Routing tables\ndefault 192.168.31.1 UGScg en0\n";
        let lines = |t: &str| t.lines().any(|line| {
            let cols: Vec<&str> = line.split_whitespace().collect();
            match cols.as_slice() {
                [dest, _gw, _flags, netif, ..] => is_default_route(dest) && netif.starts_with("utun"),
                _ => false,
            }
        });
        assert!(lines(captured));
        assert!(!lines(lost));
    }

    #[test]
    fn native_v6_default_needs_physical_default_route() {
        let noise = concat!(
            "Internet6 Destination Gateway Flags Netif Expire\n",
            "default fe80::%utun0 UGcIg utun0\n",
            "default fe80::%utun1 UGcIg utun1\n",
            "::/1 fdfe:dcba:9876::1 UGScg utun4\n",
            "8000::/1 fdfe:dcba:9876::1 UGScg utun4\n",
        );
        assert!(!native_v6_default(noise));
        assert!(native_v6_default(&format!(
            "{noise}default fe80::1@en0 UGcI en0\n"
        )));
        assert!(native_v6_default(&format!(
            "{noise}default 2408:80f1:21:c07f::1 UGSc en0\n"
        )));
        assert!(!native_v6_default(""));
        assert!(!native_v6_default("default fe80::1@en0\n"));
    }

    use super::*;

    #[test]
    fn detects_mihomo_default_route_shards() {
        for route in ["1", "2/7", "4/6", "8/5", "16/4", "32/3", "64/2", "128.0/1"] {
            assert!(is_default_route(route), "missing {route}");
        }
    }

    #[test]
    fn ignores_private_lan_routes() {
        assert!(!is_default_route("192.168.0/16"));
        assert!(!is_default_route("192.168.31"));
    }

    /// 回归（实测踩坑）：en0 上的正常网关默认路由不是"其他隧道"，
    /// 物理接口的默认路由不应触发冲突报错。
    #[test]
    fn physical_default_route_is_not_a_conflict() {
        // 模拟 ensure_route_available 的判定逻辑
        let conflict = |destination: &str, interface: &str| {
            is_default_route(destination) && interface.starts_with("utun")
        };
        assert!(!conflict("default", "en0"));
        assert!(!conflict("128.0/1", "en0"));
        assert!(conflict("default", "utun7"));
        assert!(conflict("128.0/1", "utun1024"));
    }
}

/// Controller readiness alone does not prove that the TUN listener started.
/// Mihomo logs route and interface failures after the controller comes up.
fn wait_tun_listener(
    state: &AppState,
    previous_routes: &BTreeSet<(String, String)>,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let log_path = state.data_dir.join("runtime/core-stdio.log");
    loop {
        if let Ok(log) = std::fs::read_to_string(&log_path) {
            if let Some(line) = log
                .lines()
                .find(|line| line.contains("Start TUN listening error"))
            {
                let detail = line.split("msg=").last().unwrap_or(line).trim_matches('"');
                return Err(format!("TUN 接管失败：{detail}"));
            }
        }
        match state.core().status() {
            crate::CoreStatus::Crashed(error) => return Err(error),
            crate::CoreStatus::Stopped => return Err("TUN 内核在路由就绪前已停止".into()),
            _ => {}
        }
        // A ready controller does not prove that the virtual interface took
        // over traffic. Both supported cores add IPv4 routes through utun.
        if utun_routes()?.difference(previous_routes).next().is_some() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("TUN 内核已启动，但未检测到新的 utun 路由；请检查内核日志".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Crossbow's own system proxy may share the running core with TUN. A proxy
/// left by another client is still suspended so it cannot bypass this tunnel.
pub fn toggle_tun(state: &AppState, enable: bool) -> Result<(), String> {
    // Serialize the entire transition with core start/restart and the health
    // monitor. The saved flag must never get ahead of a failed stop.
    let _operation = state.core_op.lock().unwrap_or_else(|e| e.into_inner());
    state.exit.ensure_open()?;
    toggle_tun_locked(state, enable)
}

/// 健康检查重建立。先拿到与开关相同的锁，再读标志。
/// 用户已经关掉时直接返回，避免「关了又被打开」。
pub fn reestablish_if_enabled(state: &AppState) -> Result<(), String> {
    let _operation = state.core_op.lock().unwrap_or_else(|e| e.into_inner());
    state.exit.ensure_open()?;
    let enabled = {
        let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store.data().engine.tun_enable
    };
    if !enabled {
        return Ok(());
    }
    state.sysproxy.ensure_unattended_tun_rebuild()?;
    ensure_unattended_root(state)?;
    toggle_tun_locked(state, false)?;
    toggle_tun_locked(state, true)
}

/// 连通性失败时关闭。持锁后再读，避免关掉用户刚刚打开的 TUN。
pub fn disable_if_enabled(state: &AppState) -> Result<(), String> {
    let _operation = state.core_op.lock().unwrap_or_else(|e| e.into_inner());
    state.exit.ensure_open()?;
    let enabled = {
        let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store.data().engine.tun_enable
    };
    if !enabled {
        return Ok(());
    }
    state.sysproxy.ensure_unattended_tun_restore()?;
    ensure_unattended_root(state)?;
    toggle_tun_locked(state, false)
}

/// 自检不能触发新授权，预检必须发生在关闭 TUN 或停止内核之前。
fn ensure_unattended_root(state: &AppState) -> Result<(), String> {
    if !is_setuid(&state.core().binary_path()) {
        return Err("TUN 内核需要重新授权，请手动操作 TUN 开关".into());
    }
    Ok(())
}

fn toggle_tun_locked(state: &AppState, enable: bool) -> Result<(), String> {
    let previous_tun = {
        let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store.data().engine.tun_enable
    };
    if previous_tun == enable {
        return Ok(());
    }
    let previous_routes = if enable {
        ensure_route_available()?
    } else {
        BTreeSet::new()
    };
    let was_running = matches!(
        state.core().status(),
        crate::CoreStatus::Running | crate::CoreStatus::Starting
    );
    let previous_proxy = state.sysproxy.status();
    // Preserve Crossbow's own proxy when both switches are on. If another
    // client owns the proxy, keep its exact settings for rollback/TUN exit.
    let suspended_proxy = if enable && !previous_proxy.enabled {
        Some(state.sysproxy.suspend_for_tun()?)
    } else {
        None
    };
    // 热切换优先（#23 关闭链路根治）：内核在跑、控制器可用且当前是
    // mihomo（sing-box 的 clash_api 不支持 tun 热更新）时，直接
    // PATCH /configs 切换 tun——内核不重启，没有杀 root 内核/端口
    // 交接/重启探测，毫秒级完成。关闭后内核继续以授权副本运行
    // （非 TUN 模式），下次开 TUN 也可直接热开。
    let core_binary = state.core().binary_path();
    let hot_capable = was_running
        && state.core().controller().is_some()
        && crate::core_download::is_core_bin(&core_binary, "mihomo")
        // 热开启要求内核已是 root 授权副本——普通内核热开 utun 会
        // EPERM（实测：configure tun interface: operation not permitted）
        && (!enable || crate::tun::is_setuid(&core_binary));
    if hot_capable {
        if let Some((port, secret)) = state.core().controller() {
            let client = crate::mihomo_api::Controller { port, secret };
            match client.patch_tun_enable(enable) {
                Ok(()) if enable => {
                    if let Err(error) = wait_tun_listener(state, &previous_routes) {
                        // 回退热开并落回重启链换 root 副本重试（保留已挂起的
                        // 系统代理快照，由重启链继续管理）
                        let _ = client.patch_tun_enable(false);
                        eprintln!("tun hot-enable failed to take effect, falling back to restart: {error}");
                    } else {
                        {
                            let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
                            store.data_mut().engine.tun_enable = enable;
                            store.save().map_err(|e| e.to_string())?;
                        }
                        if !previous_proxy.enabled {
                            let _ = state.sysproxy.suspend_for_tun();
                        }
                        return Ok(());
                    }
                    // 热开失败：落回下方重启链换 root 副本重试
                }
                Ok(()) => {
                    {
                        let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
                        store.data_mut().engine.tun_enable = enable;
                        store.save().map_err(|e| e.to_string())?;
                    }
                    let _ = state.sysproxy.restore_after_tun();
                    return Ok(());
                }
                Err(error) => {
                    eprintln!("tun hot-toggle via controller failed, fallback to restart: {error}");
                }
            }
        }
    }
    if was_running && !state.core().stop() {
        if let Some(suspended) = suspended_proxy {
            state.sysproxy.restore_after_failed_tun(suspended)?;
        }
        return Err("旧内核未退出，TUN 状态未改变；请重试切换".into());
    }
    {
        let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        store.data_mut().engine.tun_enable = enable;
        if let Err(error) = store.save() {
            store.data_mut().engine.tun_enable = previous_tun;
            drop(store);
            if was_running {
                let _ = crate::start_core_locked(state);
            }
            if let Some(suspended) = suspended_proxy {
                state.sysproxy.restore_after_failed_tun(suspended)?;
            }
            return Err(error.to_string());
        }
    }
    // 开启 TUN 时即使核心原本停止，也必须启动核心，否则 UI 会显示 TUN 已开，
    // 但实际没有 utun、DNS 或日志；核心已运行时则重启以应用新配置。
    // On exit from TUN, start a regular core before restoring a proxy that
    // may point at Crossbow's mixed port, even if the TUN core already died.
    let should_start = was_running || enable || previous_tun;
    if should_start {
        let start_result = crate::start_core_locked(state)
            .and_then(|()| crate::wait_core_running(state))
            .and_then(|()| {
                if enable {
                    wait_tun_listener(state, &previous_routes)
                } else {
                    Ok(())
                }
            });
        if let Err(start_error) = start_result {
            if !enable {
                // The old TUN core has already exited. Rolling the preference
                // back to true would claim a tunnel exists when there is no
                // route or process. Restore a safe external proxy instead.
                let _ = state.core().stop();
                let proxy_result = if previous_proxy.enabled {
                    state.sysproxy.disable()
                } else {
                    state.sysproxy.recover_tun_without_core()
                };
                return Err(match proxy_result {
                    Ok(()) => {
                        format!("TUN 已关闭，但普通内核启动失败；已恢复原系统代理：{start_error}")
                    }
                    Err(error) => format!(
                        "TUN 已关闭，但普通内核启动失败：{start_error}；恢复原系统代理失败：{error}"
                    ),
                });
            }
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
                .then(|| crate::start_core_locked(state).err())
                .flatten();
            let proxy_rollback_error = if previous_proxy.enabled && recovery_error.is_some() {
                // A proxy pointing at a stopped core would leave the system
                // offline. Restore the settings from before Crossbow instead.
                state.sysproxy.disable().err()
            } else {
                suspended_proxy
                    .and_then(|suspended| state.sysproxy.restore_after_failed_tun(suspended).err())
            };
            let details = [rollback_error, recovery_error, proxy_rollback_error]
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
    if !enable {
        state.sysproxy.restore_after_tun()?;
    }
    Ok(())
}
