//! 连接进程补齐：mihomo 的进程识别在新版 macOS 全失效、Windows UDP 场景
//! 竞态失败（上游 mihomo#1800），connections 快照里大量 process 为空。
//! 这里按「本地源端口 → PID → 进程名」批量反查补齐（#418）。
//!
//! macOS：一次 `lsof -Fpcn -nP -iTCP -sTCP:ESTABLISHED` 全表拉取后按端口
//! 过滤（解析器为纯函数，可离线测试）；Windows：GetExtendedTcpTable/
//! GetExtendedUdpTable 一次拉全表。查询失败静默返回空表——展示层保持
//! 现状（未知徽标），绝不因此打断快照推送。

use std::collections::HashMap;

#[cfg(target_os = "macos")]
use std::process::Command;

/// lsof `-Fpcn` 输出解析：记录块 p<PID> c<命令> n<本地>-><远端>，
/// 返回「本地端口 → 进程名」。纯函数，测试直喂样例。
fn parse_lsof_output(text: &str) -> HashMap<u16, String> {
    let mut map: HashMap<u16, String> = HashMap::new();
    let mut pid: Option<String> = None;
    let mut cmd: Option<String> = None;
    for line in text.lines() {
        let mut chars = line.chars();
        let Some(tag) = chars.next() else { continue };
        let value: String = chars.collect();
        match tag {
            'p' => pid = Some(value),
            'c' => cmd = Some(value),
            'n' => {
                // pid 只用于确认记录块完整（lsof 输出 p 行在 n 之前）；
                // 端口→进程名映射本身只需要 c 行的命令名。
                let (Some(_pid), Some(cmd)) = (pid.as_deref(), cmd.as_deref()) else {
                    continue;
                };
                // n 形如 [addr]:port->[addr]:port 或 addr:port（LISTEN 无箭头）
                let local = value.split("->").next().unwrap_or("");
                let Some(port) = local.rsplit(':').next().and_then(|p| p.parse().ok()) else {
                    continue;
                };
                map.entry(port).or_insert_with(|| cmd.to_string());
            }
            _ => {}
        }
    }
    map
}

#[cfg(target_os = "macos")]
pub fn resolve_processes(ports: &[u16]) -> HashMap<u16, String> {
    if ports.is_empty() {
        return HashMap::new();
    }
    let Ok(output) = Command::new("/usr/sbin/lsof")
        .args(["-Fpcn", "-nP", "-iTCP", "-sTCP:ESTABLISHED"])
        .output()
    else {
        return HashMap::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let full = parse_lsof_output(&text);
    ports
        .iter()
        .filter_map(|p| full.get(p).map(|name| (*p, name.clone())))
        .collect()
}

#[cfg(windows)]
pub fn resolve_processes(ports: &[u16]) -> HashMap<u16, String> {
    windows_resolve::resolve_processes(ports)
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn resolve_processes(_ports: &[u16]) -> HashMap<u16, String> {
    HashMap::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsof_parser_maps_local_ports_to_commands() {
        let sample = "p667\ncidentityservicesd\nf37\nn[fe80::1]:1024->[fe80::2]:1024\n\
                      p774\ncBrowser Helper (Aperitif)\nf14\nn127.0.0.1:59478->[20.42.65.92]:443\n\
                      p50032\ncmihomo\nf20\nn127.0.0.1:7898->127.0.0.1:59478\n";
        let map = parse_lsof_output(sample);
        assert_eq!(
            map.get(&1024).map(String::as_str),
            Some("identityservicesd")
        );
        assert_eq!(
            map.get(&59478).map(String::as_str),
            Some("Browser Helper (Aperitif)")
        );
        // mihomo 自身的出站端口也会入表；调用方负责按源端口挑选并排除自身
        assert_eq!(map.get(&7898).map(String::as_str), Some("mihomo"));
        assert!(!map.contains_key(&9999));
    }

    #[test]
    fn lsof_parser_tolerates_empty_and_malformed_lines() {
        assert!(parse_lsof_output("").is_empty());
        let sample = "pabc\nc\nn[::]:notaport\nn127.0.0.1:\np1\ncx\nn[fe80::1%en0]:55->[]:80\n";
        let map = parse_lsof_output(sample);
        assert_eq!(map.get(&55).map(String::as_str), Some("x"));
    }

    #[test]
    fn resolve_processes_empty_input_is_noop() {
        assert!(resolve_processes(&[]).is_empty());
    }

    // macOS 实机冒烟：本机必然有 ESTABLISHED 连接；不出现即视为环境受限跳过。
    #[test]
    #[cfg(target_os = "macos")]
    fn resolve_processes_smoke_on_live_table() {
        let probe = Command::new("/usr/sbin/lsof")
            .args(["-Fpcn", "-nP", "-iTCP", "-sTCP:ESTABLISHED"])
            .output()
            .expect("lsof exists on macOS");
        let text = String::from_utf8_lossy(&probe.stdout);
        let ports: Vec<u16> = parse_lsof_output(&text).keys().copied().take(5).collect();
        if ports.is_empty() {
            return; // CI 沙盒可能无外连
        }
        let resolved = resolve_processes(&ports);
        assert!(!resolved.is_empty(), "已知活跃端口应能解析出进程名");
    }

    /// #418 端到端（环境门控）：本机 Crossbow 内核在跑时，从 controller
    /// 拉真实 connections，对缺进程连接跑反查——命中率必须显著（mihomo
    /// 失效场景下这些全是应用层连接）。内核没跑/无 descriptor 则跳过。
    #[test]
    #[cfg(target_os = "macos")]
    fn resolve_processes_against_live_core_snapshot() {
        let Ok(home) = std::env::var("HOME") else {
            return;
        };
        let Ok(descriptor) = std::fs::read_to_string(format!(
            "{home}/Library/Application Support/com.richard.crossbow/runtime/core-descriptor.json"
        )) else {
            return; // 无运行中内核
        };
        let Ok(d) = serde_json::from_str::<serde_json::Value>(&descriptor) else {
            return;
        };
        let (Some(port), Some(secret)) = (
            d["port"].as_u64().map(|p| p as u16),
            d["secret"].as_str().map(String::from),
        ) else {
            return;
        };
        // ureq 的 call 是 ureq::Error，into_string 是 io::Error——错误类型
        // 不同不能 and_then 串，分两步 let-else。
        let Ok(response) = ureq::get(&format!("http://127.0.0.1:{port}/connections"))
            .set("Authorization", &format!("Bearer {secret}"))
            .timeout(std::time::Duration::from_secs(5))
            .call()
        else {
            return; // 内核不在跑
        };
        let Ok(body) = response.into_string() else {
            return;
        };
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let Some(conns) = v["connections"].as_array() else {
            return;
        };
        let missing: Vec<u16> = conns
            .iter()
            .filter(|c| {
                c["metadata"]["processPath"]
                    .as_str()
                    .unwrap_or("")
                    .is_empty()
            })
            .filter_map(|c| {
                c["metadata"]["sourcePort"]
                    .as_str()
                    .and_then(|s| s.parse().ok())
            })
            .filter(|p| *p != 0)
            .collect();
        if missing.is_empty() {
            return;
        }
        let resolved = resolve_processes(&missing);
        let hit = missing.iter().filter(|p| resolved.contains_key(p)).count();
        assert!(
            hit * 4 >= missing.len(),
            "实机反查命中率过低：{hit}/{}，样例 {:?}",
            missing.len(),
            &missing[..missing.len().min(5)]
        );
    }
}

// Windows 实现：一次拉全表后过滤（ IpHelper 的 TCP/UDP 表，windows-sys 直链）。
#[cfg(windows)]
mod windows_resolve {
    use super::*;
    use std::mem;

    const UDP_PROC_NONE: &str = "";

    unsafe fn pid_to_name(pid: u32) -> String {
        if pid == 0 {
            return UDP_PROC_NONE.to_string();
        }
        let handle = windows_sys::Win32::System::Threading::OpenProcess(
            windows_sys::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        );
        if handle == 0 {
            return UDP_PROC_NONE.to_string();
        }
        let mut buf = [0u16; 1024];
        let mut written = 0u32;
        let ok = windows_sys::Win32::System::Threading::QueryFullProcessImageNameW(
            handle,
            0,
            buf.as_mut_ptr(),
            &mut written,
        );
        windows_sys::Win32::Foundation::CloseHandle(handle);
        if ok == 0 || written == 0 {
            return UDP_PROC_NONE.to_string();
        }
        let path = String::from_utf16_lossy(&buf[..written as usize]);
        path.rsplit(['\\', '/']).next().unwrap_or("").to_string()
    }

    /// dwLocalPort 是网络字节序的 u32，端口在其低 16 位（ntohs 语义）。
    #[inline]
    fn local_port(raw: u32) -> u16 {
        (raw as u16).swap_bytes()
    }

    pub fn resolve_processes(ports: &[u16]) -> HashMap<u16, String> {
        let wanted: std::collections::HashSet<u16> = ports.iter().copied().collect();
        let mut map: HashMap<u16, String> = HashMap::new();
        unsafe {
            // TCP：MIB_TCPTABLE_OWNER_PID，一次全表
            let mut size = 0u32;
            let class = windows_sys::Win32::NetworkManagement::IpHelper::TCP_TABLE_OWNER_PID_ALL;
            let _ = windows_sys::Win32::NetworkManagement::IpHelper::GetExtendedTcpTable(
                None, &mut size, 0, 2, class, 0,
            );
            if size > 0 {
                let mut buf = vec![0u8; size as usize];
                let ret = windows_sys::Win32::NetworkManagement::IpHelper::GetExtendedTcpTable(
                    buf.as_mut_ptr() as *mut _,
                    &mut size,
                    0,
                    2,
                    class,
                    0,
                );
                if ret == 0 {
                    let table = &*(buf.as_ptr() as *const windows_sys::Win32::NetworkManagement::IpHelper::MIB_TCPTABLE_OWNER_PID);
                    let entries = std::slice::from_raw_parts(
                        &table.table[0] as *const _,
                        table.dwNumEntries as usize,
                    );
                    for e in entries {
                        let p = local_port(e.dwLocalPort);
                        if wanted.contains(&p) && !map.contains_key(&p) {
                            map.insert(p, pid_to_name(e.dwOwningPid));
                        }
                    }
                }
            }
            // UDP：MIB_UDPTABLE_OWNER_PID 同型
            let mut udp_size = 0u32;
            let uclass = windows_sys::Win32::NetworkManagement::IpHelper::UDP_TABLE_OWNER_PID;
            let _ = windows_sys::Win32::NetworkManagement::IpHelper::GetExtendedUdpTable(
                None,
                &mut udp_size,
                0,
                2,
                uclass,
                0,
            );
            if udp_size > 0 {
                let mut buf = vec![0u8; udp_size as usize];
                let ret = windows_sys::Win32::NetworkManagement::IpHelper::GetExtendedUdpTable(
                    buf.as_mut_ptr() as *mut _,
                    &mut udp_size,
                    0,
                    2,
                    uclass,
                    0,
                );
                if ret == 0 {
                    let table = &*(buf.as_ptr() as *const windows_sys::Win32::NetworkManagement::IpHelper::MIB_UDPTABLE_OWNER_PID);
                    let entries = std::slice::from_raw_parts(
                        &table.table[0] as *const _,
                        table.dwNumEntries as usize,
                    );
                    for e in entries {
                        let p = local_port(e.dwLocalPort);
                        if wanted.contains(&p) && !map.contains_key(&p) {
                            map.insert(p, pid_to_name(e.dwOwningPid));
                        }
                    }
                }
            }
        }
        map
    }
}
