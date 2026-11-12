//! 与其他代理/VPN 的共存检测（#23）。
//!
//! 启动/首页轮询时扫描两类冲突源，发现即由首页横幅提示（不静默）：
//! 1. 其他隧道：utun 接口持有默认路由（其他代理/VPN 的 TUN 正在接管）
//! 2. 常驻代理软件：已知特权服务（clash-verge-service 等）与本应用无关的
//!    内核进程（mihomo/sing-box，路径不属于本应用）
//!
//! 检测是信息性的——真正的硬冲突由开 TUN 时的 `ensure_route_available`
//! 与端口预检的自愈/报错兜底。

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct CoexistFinding {
    /// tunnel = 其他 TUN 接管默认路由；service = 已知常驻服务；core = 其他内核进程
    pub kind: String,
    pub detail: String,
}

/// 判定 netstat 目的地列是否为全量默认路由。
/// macOS 把 0.0.0.0/0 渲染为 default，mihomo/sing-box 的 auto-route
/// 渲染为 1、2/7、4/6 … 128.0/1 的分流系列。
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn is_default_destination(destination: &str) -> bool {
    matches!(destination, "default" | "0" | "0/0" | "0.0.0.0/0")
        || matches!(
            destination,
            "1" | "1/8"
                | "1.0.0.0/8"
                | "2/7"
                | "4/6"
                | "8/5"
                | "16/4"
                | "32/3"
                | "64/2"
                | "128.0/1"
        )
}

/// 本应用内核的路径特征（不视为“其他代理”）。
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn is_own_core_path(line: &str) -> bool {
    line.contains("/Crossbow.app/")
        || line.contains("com.richard.crossbow")
        || line.contains("com.crossbow.app")
}

/// 从 `ps -axo command=` 的行里识别其他代理软件。
/// 返回 (是否发现已知常驻服务, 其他内核进程命令行列表)。
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn scan_process_lines(lines: &[&str]) -> (bool, Vec<String>) {
    let mut service = false;
    let mut cores: Vec<String> = Vec::new();
    for raw in lines {
        let line = raw.trim();
        if line.is_empty() || line.contains("grep ") {
            continue;
        }
        if line.contains("clash-verge-service") {
            service = true;
            continue;
        }
        let is_core =
            (line.contains("mihomo") || line.contains("sing-box")) && !is_own_core_path(line);
        if is_core && !cores.iter().any(|seen| seen == line) {
            cores.push(line.to_string());
        }
    }
    (service, cores)
}

/// 由三类原始信号组装提示：**只提示真实冲突源**——闲置的常驻服务
/// （未拉起内核）不占端口不接管流量，单独提示只会狼来了；其内核
/// （verge-mihomo）跑起来时自然会以 core 条目出现。
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn build_findings(
    tunnel: Option<String>,
    service: bool,
    cores: Vec<String>,
) -> Vec<CoexistFinding> {
    let mut out = Vec::new();
    if let Some(iface) = tunnel {
        out.push(CoexistFinding {
            kind: "tunnel".into(),
            detail: format!("接口 {iface} 正在接管默认路由（其他代理/VPN 的 TUN 已开启）"),
        });
    }
    if !cores.is_empty() {
        out.push(CoexistFinding {
            kind: "core".into(),
            detail: format!("检测到其他代理的内核进程：{}", cores.join(" ｜ ")),
        });
    }
    let _ = service;
    out
}

/// Only exclude the exact interface confirmed by Crossbow's live controller.
/// Continue scanning after an own route so a second VPN is still reported.
#[cfg(any(target_os = "macos", test))]
fn foreign_tunnel(table: &str, own_device: Option<&str>) -> Option<String> {
    table.lines().find_map(|line| {
        let cols: Vec<&str> = line.split_whitespace().collect();
        let destination = *cols.first()?;
        // Netif is column 4; the optional Expire column may follow it.
        let interface = *cols.get(3)?;
        (is_default_destination(destination)
            && interface.starts_with("utun")
            && Some(interface) != own_device)
            .then(|| interface.to_string())
    })
}

/// macOS：检测外部隧道与内核，排除本应用控制器确认的 TUN 接口。
#[cfg(target_os = "macos")]
pub fn detect(state: &crate::AppState) -> Vec<CoexistFinding> {
    let core = state.core();
    let own_device = if core.status() == crate::CoreStatus::Running {
        core.controller().and_then(|(port, secret)| {
            crate::engine::api::Controller { port, secret }
                .active_tun_device()
                .ok()
                .flatten()
        })
    } else {
        None
    };

    // Read ownership before the route table, without holding an AppState lock
    // across controller I/O. A failed query never exempts all utun interfaces.
    let tunnel = crate::process::command("netstat")
        .args(["-rn", "-f", "inet"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| {
            foreign_tunnel(
                &String::from_utf8_lossy(&output.stdout),
                own_device.as_deref(),
            )
        });

    // 2) 常驻服务与其他内核进程
    let (service, cores) = crate::process::command("ps")
        .args(["-axo", "command="])
        .output()
        .map(|output| {
            let text = String::from_utf8_lossy(&output.stdout);
            scan_process_lines(&text.lines().collect::<Vec<_>>())
        })
        .unwrap_or((false, Vec::new()));

    let mut findings = build_findings(tunnel, service, cores);

    // 3) Tailscale（#145）：exit node 接管默认路由 = 与 TUN 互斥（横幅）；
    //    纯组网（tailscaled 在跑、无默认路由接管）= 兼容（信息性）
    if let Some(text) = command_stdout("netstat", &["-rn", "-f", "inet"]) {
        if tailscale_exit_node(&text) {
            findings.push(CoexistFinding {
                kind: "tunnel".into(),
                detail: "检测到 Tailscale 出口节点接管默认路由，与 TUN 互斥；请在 Tailscale 关闭出口节点".into(),
            });
        }
    }

    findings
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn tailscale_exit_node(text: &str) -> bool {
    text.lines().any(|line| {
        let cols: Vec<&str> = line.split_whitespace().collect();
        match cols.as_slice() {
            [dest, _gw, _flags, netif, ..] => {
                *dest == "default" && netif.eq_ignore_ascii_case("Tailscale")
            }
            _ => false,
        }
    })
}

/// Windows：tasklist 识别其他代理内核与常驻服务，Get-NetAdapter 识别
/// 其他隧道驱动适配器。本应用侧车为 mihomo-x86_64-pc-windows-msvc.exe
/// 全名、TUN 设备将命名为 Crossbow，与第三方 mihomo.exe/verge-mihomo.exe
/// 及其 wintun 适配器不冲突，可按名直分。#82
#[cfg(target_os = "windows")]
pub fn detect(_state: &crate::AppState) -> Vec<CoexistFinding> {
    let mut findings = Vec::new();

    // 1) 其他代理内核进程（#88）
    if let Some(text) = command_stdout("tasklist", &["/FO", "CSV", "/NH"]) {
        let cores = foreign_core_images(&text);
        if !cores.is_empty() {
            findings.push(CoexistFinding {
                kind: "core".into(),
                detail: format!("检测到其他代理的内核进程：{}", cores.join(" ｜ ")),
            });
        }
    }

    // 2) 常驻服务：信息性（kind=service 不触发首页横幅），仅在隧道等
    //    真冲突出现时随 details 一起给出上下文——对齐 macOS 的降噪策略
    if let Some(text) = command_stdout("tasklist", &["/FO", "CSV", "/NH", "/svc"]) {
        let services = foreign_service_names(&text);
        if !services.is_empty() {
            findings.push(CoexistFinding {
                kind: "service".into(),
                detail: format!("检测到其他代理的常驻服务：{}", services.join(" ｜ ")),
            });
        }
    }

    // 3) 其他隧道驱动适配器：外来 wintun/TAP 处于 Up = 大概率正接管流量，
    //    与 macOS「外部 utun 持默认路由」同级，触发横幅
    if let Some(text) = command_stdout(
        "powershell",
        &[
            "-NoProfile",
            "-Command",
            "Get-NetAdapter | Where-Object Status -eq 'Up' | Select-Object Name,InterfaceDescription | ConvertTo-Csv -NoTypeInformation",
        ],
    ) {
        let adapters = foreign_tunnel_adapters(&text);
        if !adapters.is_empty() {
            findings.push(CoexistFinding {
                kind: "tunnel".into(),
                detail: format!("检测到其他隧道适配器：{}", adapters.join(" ｜ ")),
            });
        }
    }

    // 4) Tailscale（#145）：exit node（0.0.0.0/0 经 Tailscale 适配器）与 TUN
    //    互斥 → 横幅；纯组网 → 信息性
    if let Some(text) = command_stdout(
        "powershell",
        &[
            "-NoProfile",
            "-Command",
            "Get-NetRoute -DestinationPrefix '0.0.0.0/0' | Select-Object -ExpandProperty InterfaceAlias",
        ],
    ) {
        if text
            .lines()
            .any(|l| l.trim().eq_ignore_ascii_case("Tailscale"))
        {
            findings.push(CoexistFinding {
                kind: "tunnel".into(),
                detail: "检测到 Tailscale 出口节点接管默认路由，与 TUN 互斥；请在 Tailscale 关闭出口节点".into(),
            });
        }
    }

    findings
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn command_stdout(program: &str, args: &[&str]) -> Option<String> {
    let output = crate::process::command(program).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// tasklist CSV 首列为图像名；命中其他代理特征且排除本应用
#[cfg(any(target_os = "windows", test))]
fn foreign_core_images(text: &str) -> Vec<String> {
    let mut cores: Vec<String> = Vec::new();
    for line in text.lines() {
        let Some(image) = line.split('"').nth(1) else {
            continue;
        };
        let lower = image.to_lowercase();
        let is_other_proxy =
            (lower.contains("verge") || lower == "mihomo.exe" || lower == "sing-box.exe")
                && !lower.contains("crossbow");
        if is_other_proxy && !cores.iter().any(|c| c.contains(&lower)) {
            cores.push(image.to_string());
        }
    }
    cores
}

/// tasklist /svc CSV：第 3 列起为进程承载的服务名，识别其他代理常驻服务
#[cfg(any(target_os = "windows", test))]
fn foreign_service_names(text: &str) -> Vec<String> {
    let mut services: Vec<String> = Vec::new();
    for line in text.lines() {
        let quoted: Vec<&str> = line.split('"').collect();
        let Some(image) = quoted.get(1) else { continue };
        if image.to_lowercase().contains("crossbow") {
            continue;
        }
        // split('"') 后奇数位是带引号字段：1=图像名，3=PID，5 起为服务名
        for field in quoted.iter().skip(5).step_by(2) {
            let lower = field.to_lowercase();
            let is_other_service =
                (lower.contains("clash") || lower.contains("verge") || lower.contains("sing-box"))
                    && !lower.contains("crossbow");
            if is_other_service && !services.iter().any(|s| s.eq_ignore_ascii_case(field)) {
                services.push((*field).to_string());
            }
        }
    }
    services
}

/// Get-NetAdapter CSV（Name,InterfaceDescription）：名称/描述命中隧道
/// 驱动特征即外来隧道；本应用设备名含 Crossbow，先行排除
#[cfg(any(target_os = "windows", test))]
fn foreign_tunnel_adapters(text: &str) -> Vec<String> {
    const TUNNEL_PATTERNS: [&str; 5] = ["wintun", "tap", "wireguard", "openvpn", "tunnel"];
    let mut adapters: Vec<String> = Vec::new();
    for line in text.lines() {
        let quoted: Vec<&str> = line.split('"').collect();
        let (Some(name), Some(desc)) = (quoted.get(1), quoted.get(3)) else {
            continue;
        };
        // ConvertTo-Csv 的表头行
        if name.eq_ignore_ascii_case("Name") {
            continue;
        }
        let haystack = format!("{name} {desc}").to_lowercase();
        let is_tunnel_driver = TUNNEL_PATTERNS.iter().any(|p| haystack.contains(p));
        // Tailscale 单独判定：纯组网不冲突（降噪），exit node 由下方
        // 0.0.0.0/0 路由检查负责（#145）
        let is_tailscale = haystack.contains("tailscale");
        let is_ours = haystack.contains("crossbow");
        if is_tunnel_driver
            && !is_tailscale
            && !is_ours
            && !adapters.iter().any(|a| a.eq_ignore_ascii_case(name))
        {
            adapters.push(format!("{name}（{desc}）"));
        }
    }
    adapters
}

#[cfg(target_os = "linux")]
pub fn detect(state: &crate::AppState) -> Vec<CoexistFinding> {
    let session = crate::network::linux_service::status().ok().flatten();
    let own_index = session.as_ref().filter(|s| s.active).map(|s| s.ifindex);
    let wireguard_coexist = state
        .store
        .lock()
        .unwrap()
        .data()
        .engine
        .linux_wireguard_coexist;
    let mut pids: Vec<_> = state.core().pid().into_iter().collect();
    if let Some(session) = session {
        pids.push(session.pid);
    }
    match crossbow_service::linux_network::conflicts_with_wireguard(
        own_index,
        &pids,
        wireguard_coexist,
    ) {
        Ok(conflicts) => conflicts
            .into_iter()
            .map(|detail| CoexistFinding {
                kind: "tunnel".into(),
                detail,
            })
            .collect(),
        Err(detail) => vec![CoexistFinding {
            kind: "tunnel".into(),
            detail,
        }],
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
pub fn detect(_state: &crate::AppState) -> Vec<CoexistFinding> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWN_ROUTES: &str = "Destination Gateway Flags Netif Expire\ndefault 192.168.1.1 UGScg en0\n1 198.18.0.1 UGSc utun4\n2/7 198.18.0.1 UGSc utun4\n128.0/1 198.18.0.1 UGSc utun4\n";

    /// #82：Windows tasklist 普通行识别其他代理内核，排除本应用
    #[test]
    fn windows_core_images_match_and_exclude_own() {
        let text = concat!(
            "\"verge-mihomo.exe\",\"123\",\"Console\",\"1\",\"12,345 K\"\n",
            "\"mihomo-x86_64-pc-windows-msvc.exe\",\"456\",\"Console\",\"1\",\"9,876 K\"\n",
            "\"sing-box.exe\",\"789\",\"Console\",\"1\",\"1,234 K\"\n",
            "\"explorer.exe\",\"1024\",\"Console\",\"1\",\"45,678 K\"\n",
        );
        let cores = foreign_core_images(text);
        assert_eq!(cores, vec!["verge-mihomo.exe", "sing-box.exe"]);
    }

    /// #82：tasklist /svc 第 5 个奇数位起为服务名；本应用服务被排除
    #[test]
    fn windows_service_names_from_svc_csv() {
        let text = concat!(
            "\"clash-verge-service.exe\",\"900\",\"clash_verge_service\",\"\",\n",
            "\"crossbow-service.exe\",\"901\",\"crossbow_service\",\"\",\n",
            "\"svchost.exe\",\"902\",\"TermService\",\"SessionEnv\"\n",
        );
        let services = foreign_service_names(text);
        assert_eq!(services, vec!["clash_verge_service"]);
    }

    /// #82：Get-NetAdapter CSV——外来隧道驱动 Up 适配器命中；本应用
    /// Crossbow 适配器与物理网卡不误报
    #[test]
    fn windows_tunnel_adapters_match_and_exclude_own() {
        let text = concat!(
            "\"Name\",\"InterfaceDescription\"\n",
            "\"以太网\",\"Realtek PCIe GbE Family Controller\"\n",
            "\"mihomo\",\"Wintun Userspace Tunnel\"\n",
            "\"Crossbow\",\"Wintun Userspace Tunnel\"\n",
            "\"WireGuard\",\"WireGuard Tunnel\"\n",
        );
        let adapters = foreign_tunnel_adapters(text);
        assert_eq!(
            adapters,
            vec![
                "mihomo（Wintun Userspace Tunnel）",
                "WireGuard（WireGuard Tunnel）"
            ]
        );

        // Tailscale 不参与普通隧道误报（exit node 单独判定）
        assert!(foreign_tunnel_adapters(concat!(
            "\"Name\",\"InterfaceDescription\"\n",
            "\"Tailscale\",\"Tailscale Tunnel\"\n",
        ))
        .is_empty());
    }

    /// #145：Tailscale exit node = default 路由经 Tailscale 接口
    #[test]
    fn tailscale_exit_node_detection() {
        assert!(tailscale_exit_node("default 100.64.0.1 UGSc Tailscale\n"));
        assert!(!tailscale_exit_node("default 192.168.31.1 UGScg en0\n"));
        assert!(!tailscale_exit_node(""));
    }

    #[test]
    fn own_tun_and_idle_service_do_not_warn() {
        let (service, cores) = scan_process_lines(&[
            "/Applications/Crossbow.app/Contents/MacOS/mihomo",
            "/Library/PrivilegedHelperTools/clash-verge-service",
        ]);
        assert!(
            build_findings(foreign_tunnel(OWN_ROUTES, Some("utun4")), service, cores).is_empty()
        );
    }

    #[test]
    fn external_tun_is_found_after_own_routes() {
        let table = format!("{OWN_ROUTES}default 10.0.0.1 UGSc utun7\n");
        assert_eq!(
            foreign_tunnel(&table, Some("utun4")).as_deref(),
            Some("utun7")
        );
    }

    #[test]
    fn unconfirmed_ownership_does_not_hide_tunnels() {
        assert_eq!(foreign_tunnel(OWN_ROUTES, None).as_deref(), Some("utun4"));
        assert_eq!(
            foreign_tunnel(OWN_ROUTES, Some("utun40")).as_deref(),
            Some("utun4")
        );
    }

    #[test]
    fn netif_precedes_optional_expire_column() {
        let table = "default 10.0.0.1 UGSc utun7 123\n";
        assert_eq!(foreign_tunnel(table, None).as_deref(), Some("utun7"));
        assert!(foreign_tunnel(table, Some("utun7")).is_none());
    }

    #[test]
    fn default_destinations_recognized() {
        for d in ["default", "1", "2/7", "128.0/1", "64/2"] {
            assert!(is_default_destination(d), "missing {d}");
        }
        assert!(!is_default_destination("192.168.31"));
        assert!(!is_default_destination("224.0.0/4"));
    }

    #[test]
    fn own_kernels_not_flagged() {
        let lines = [
            "/Applications/Crossbow.app/Contents/MacOS/mihomo -d runtime",
            "/Users/x/com.richard.crossbow/tun-binaries/v1/mihomo -d runtime",
        ];
        let (service, cores) = scan_process_lines(&lines);
        assert!(!service);
        assert!(cores.is_empty());
    }

    #[test]
    fn verge_service_and_cores_detected() {
        let lines = [
            "/Library/PrivilegedHelperTools/io.github.clash-verge-rev.clash-verge-rev.service.bundle/Contents/MacOS/clash-verge-service",
            "/usr/local/bin/sing-box run -c x.json",
        ];
        let (service, cores) = scan_process_lines(&lines);
        assert!(service);
        assert_eq!(cores.len(), 1);
        assert!(cores[0].contains("sing-box"));
    }

    #[test]
    fn grep_noise_ignored() {
        let lines = ["grep --color=auto mihomo"];
        let (service, cores) = scan_process_lines(&lines);
        assert!(!service);
        assert!(cores.is_empty());
    }

    /// 策略回归（用户反馈）：闲置常驻服务不提示——退出 Verge 应用后
    /// 其特权服务仍常驻属正常状态，单独横幅是狼来了。
    #[test]
    fn idle_service_alone_not_reported() {
        let findings = build_findings(None, true, Vec::new());
        assert!(findings.is_empty());
    }

    #[test]
    fn tunnel_and_core_reported() {
        let findings = build_findings(
            Some("utun7".into()),
            true,
            vec!["/Library/.../verge-mihomo -d x".into()],
        );
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].kind, "tunnel");
        assert_eq!(findings[1].kind, "core");
    }

    #[test]
    fn tunnel_alone_reported() {
        let findings = build_findings(Some("utun9".into()), false, Vec::new());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, "tunnel");
    }
}
