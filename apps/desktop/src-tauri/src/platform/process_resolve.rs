//! Fill in process metadata for local proxy sockets when the core has no answer.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum SocketKey {
    Tcp { local: SocketAddr, peer: SocketAddr },
    Udp(SocketAddr),
}

#[derive(Debug, Eq, Hash, PartialEq)]
pub(crate) struct ConnectionSocket {
    pub id: String,
    pub socket: SocketKey,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Process {
    pub name: String,
    pub path: String,
}

type Matches = HashMap<ConnectionSocket, Process>;
const SCAN_INTERVAL: Duration = Duration::from_secs(3);

#[derive(Default)]
pub(crate) struct ProcessLookup {
    matches: Matches,
    pending: Option<Receiver<Matches>>,
    last_scan: Option<Instant>,
}

impl ProcessLookup {
    pub fn update(&mut self, mut requests: HashSet<ConnectionSocket>) -> &Matches {
        if let Some(pending) = &self.pending {
            match pending.try_recv() {
                Ok(matches) => {
                    self.matches.extend(matches);
                    self.pending = None;
                }
                Err(TryRecvError::Disconnected) => self.pending = None,
                Err(TryRecvError::Empty) => {}
            }
        }

        // Bind results to a core connection ID and both endpoints. A new connection
        // that reuses a socket address must wait for its own lookup.
        self.matches.retain(|key, _| requests.contains(key));
        requests.retain(|key| !self.matches.contains_key(key));
        if requests.is_empty()
            || self.pending.is_some()
            || self
                .last_scan
                .is_some_and(|scan| scan.elapsed() < SCAN_INTERVAL)
        {
            return &self.matches;
        }

        self.last_scan = Some(Instant::now());
        let (send, receive) = mpsc::sync_channel(1);
        if std::thread::Builder::new()
            .name("connection-process-lookup".into())
            .spawn(move || {
                let sockets = requests.iter().map(|r| r.socket).collect();
                let owners = resolve_sockets(&sockets);
                let matches = requests
                    .into_iter()
                    .filter_map(|r| owners.get(&r.socket).cloned().map(|owner| (r, owner)))
                    .collect();
                let _ = send.send(matches);
            })
            .is_ok()
        {
            self.pending = Some(receive);
        }
        &self.matches
    }
}

/// Apply the same representation to kernel metadata and OS socket tables.
pub(crate) fn socket_address(ip: std::net::IpAddr, port: u16) -> SocketAddr {
    SocketAddr::new(ip.to_canonical(), port)
}

#[cfg(any(target_os = "macos", test))]
fn parse_lsof_output(output: &str, wanted: &HashSet<SocketKey>) -> HashMap<SocketKey, Process> {
    let mut owners: HashMap<SocketKey, Option<(&str, &str)>> = HashMap::new();
    let (mut pid, mut name) = ("", "");
    // With -F0 lsof terminates fields with NUL and separates record sets by newlines.
    for field in output
        .split('\0')
        .map(|field| field.trim_start_matches('\n'))
    {
        let Some(value) = field.get(1..) else {
            continue;
        };
        match field.as_bytes()[0] {
            b'p' => {
                pid = value;
                name = "";
            }
            b'c' => name = value,
            b'n' if !pid.is_empty() && !name.is_empty() => {
                let Some((local, peer)) = value.split_once("->") else {
                    continue;
                };
                let (Ok(local), Ok(peer)) =
                    (local.parse::<SocketAddr>(), peer.parse::<SocketAddr>())
                else {
                    continue;
                };
                let key = SocketKey::Tcp {
                    local: socket_address(local.ip(), local.port()),
                    peer: socket_address(peer.ip(), peer.port()),
                };
                if wanted.contains(&key) {
                    let owner = owners.entry(key).or_insert(Some((pid, name)));
                    if owner.is_some_and(|(existing, _)| existing != pid) {
                        *owner = None;
                    }
                }
            }
            _ => {}
        }
    }
    owners
        .into_iter()
        .filter_map(|(key, owner)| {
            owner.map(|(_, name)| {
                (
                    key,
                    Process {
                        name: name.to_owned(),
                        path: String::new(),
                    },
                )
            })
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn resolve_sockets(wanted: &HashSet<SocketKey>) -> HashMap<SocketKey, Process> {
    read_lsof()
        .map(|output| parse_lsof_output(&output, wanted))
        .unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn read_lsof() -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};

    const MAX_OUTPUT: u64 = 8 * 1024 * 1024;
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut child = Command::new("/usr/sbin/lsof")
        .args(["-F0pcn", "-nP", "-iTCP", "-sTCP:ESTABLISHED"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take().expect("piped lsof stdout");
    let (send, receive) = mpsc::sync_channel(1);
    // Drain stdout while the child runs, including when output exceeds pipe capacity.
    let reader = std::thread::Builder::new().spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout.take(MAX_OUTPUT + 1).read_to_end(&mut bytes);
        let _ = send.send(
            result
                .ok()
                .filter(|&n| n as u64 <= MAX_OUTPUT)
                .map(|_| bytes),
        );
    });
    if reader.is_ok() {
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => {
                    let bytes = receive
                        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                        .ok()??;
                    return Some(String::from_utf8_lossy(&bytes).into_owned());
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                _ => break,
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    None
}

#[cfg(windows)]
use windows_resolve::resolve_sockets;

#[cfg(not(any(target_os = "macos", windows)))]
fn resolve_sockets(_wanted: &HashSet<SocketKey>) -> HashMap<SocketKey, Process> {
    HashMap::new()
}

#[cfg(windows)]
mod windows_resolve {
    use super::*;
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use std::ptr;
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, GetExtendedUdpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID,
        MIB_TCP_STATE_ESTAB, MIB_UDP6ROW_OWNER_PID, MIB_UDPROW_OWNER_PID,
        TCP_TABLE_OWNER_PID_CONNECTIONS, UDP_TABLE_OWNER_PID,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    const AF_INET: u32 = 2;
    const AF_INET6: u32 = 23;
    type TableQuery = unsafe extern "system" fn(*mut c_void, *mut u32, i32, u32, i32, u32) -> u32;

    fn process(pid: u32) -> Option<Process> {
        if pid == 0 {
            return None;
        }
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return None;
        }

        // QueryFullProcessImageNameW takes capacity on input and length on output.
        let mut buffer = vec![0u16; 32768];
        let mut length = buffer.len() as u32;
        let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut length) };
        unsafe { CloseHandle(handle) };
        if ok == 0 || length == 0 || length as usize > buffer.len() {
            return None;
        }
        let path = String::from_utf16_lossy(&buffer[..length as usize]);
        let name = path.rsplit(['\\', '/']).next()?.to_owned();
        Some(Process { name, path })
    }

    // Each owner table is a DWORD count followed by an array of C rows aligned to DWORD.
    // Callers use only the four plain-data MIB_*ROW_OWNER_PID layouts.
    unsafe fn table_rows<T: Copy>(query: TableQuery, family: u32, class: i32) -> Vec<T> {
        let mut size = 0u32;
        if unsafe { query(ptr::null_mut(), &mut size, 0, family, class, 0) }
            != ERROR_INSUFFICIENT_BUFFER
        {
            return Vec::new();
        }
        for _ in 0..2 {
            if !(4..=16 * 1024 * 1024).contains(&size) {
                break;
            }
            let capacity = size as usize;
            let mut table = vec![0u32; capacity.div_ceil(size_of::<u32>())];
            let ptr = table.as_mut_ptr();
            match unsafe { query(ptr.cast(), &mut size, 0, family, class, 0) } {
                0 => {
                    let count = table[0] as usize;
                    let len = (size as usize).min(capacity);
                    if count > len.saturating_sub(4) / size_of::<T>() {
                        return Vec::new();
                    }
                    let rows = unsafe { ptr.add(1).cast::<T>() };
                    return (0..count)
                        .map(|i| unsafe { rows.add(i).read_unaligned() })
                        .collect();
                }
                ERROR_INSUFFICIENT_BUFFER => continue,
                _ => break,
            }
        }
        Vec::new()
    }

    fn address(ip: impl Into<IpAddr>, port: u32) -> SocketAddr {
        socket_address(ip.into(), u16::from_be(port as u16))
    }

    pub(super) fn resolve_sockets(wanted: &HashSet<SocketKey>) -> HashMap<SocketKey, Process> {
        let mut owners = HashMap::new();
        let mut record = |key, pid| {
            if wanted.contains(&key) {
                let owner = owners.entry(key).or_insert(Some(pid));
                if owner.is_some_and(|existing| existing != pid) {
                    *owner = None;
                }
            }
        };

        if wanted
            .iter()
            .any(|key| matches!(key, SocketKey::Tcp { .. }))
        {
            for row in unsafe {
                table_rows::<MIB_TCPROW_OWNER_PID>(
                    GetExtendedTcpTable,
                    AF_INET,
                    TCP_TABLE_OWNER_PID_CONNECTIONS,
                )
            } {
                if row.dwState == MIB_TCP_STATE_ESTAB as u32 {
                    let local = address(
                        Ipv4Addr::from(row.dwLocalAddr.to_ne_bytes()),
                        row.dwLocalPort,
                    );
                    let peer = address(
                        Ipv4Addr::from(row.dwRemoteAddr.to_ne_bytes()),
                        row.dwRemotePort,
                    );
                    record(SocketKey::Tcp { local, peer }, row.dwOwningPid);
                }
            }
            for row in unsafe {
                table_rows::<MIB_TCP6ROW_OWNER_PID>(
                    GetExtendedTcpTable,
                    AF_INET6,
                    TCP_TABLE_OWNER_PID_CONNECTIONS,
                )
            } {
                if row.dwState == MIB_TCP_STATE_ESTAB as u32 {
                    let local = address(Ipv6Addr::from(row.ucLocalAddr), row.dwLocalPort);
                    let peer = address(Ipv6Addr::from(row.ucRemoteAddr), row.dwRemotePort);
                    record(SocketKey::Tcp { local, peer }, row.dwOwningPid);
                }
            }
        }

        let mut wildcard_ports = HashSet::new();
        if wanted.iter().any(|key| matches!(key, SocketKey::Udp(_))) {
            for row in unsafe {
                table_rows::<MIB_UDPROW_OWNER_PID>(
                    GetExtendedUdpTable,
                    AF_INET,
                    UDP_TABLE_OWNER_PID,
                )
            } {
                let local = address(
                    Ipv4Addr::from(row.dwLocalAddr.to_ne_bytes()),
                    row.dwLocalPort,
                );
                if local.ip().is_unspecified() {
                    wildcard_ports.insert(local.port());
                }
                record(SocketKey::Udp(local), row.dwOwningPid);
            }
            for row in unsafe {
                table_rows::<MIB_UDP6ROW_OWNER_PID>(
                    GetExtendedUdpTable,
                    AF_INET6,
                    UDP_TABLE_OWNER_PID,
                )
            } {
                let local = address(Ipv6Addr::from(row.ucLocalAddr), row.dwLocalPort);
                if local.ip().is_unspecified() {
                    wildcard_ports.insert(local.port());
                }
                record(SocketKey::Udp(local), row.dwOwningPid);
            }
        }

        let mut processes = HashMap::new();
        owners
            .into_iter()
            .filter_map(|(key, pid)| {
                if matches!(key, SocketKey::Udp(local) if wildcard_ports.contains(&local.port())) {
                    return None;
                }
                processes
                    .entry(pid?)
                    .or_insert_with_key(|pid| process(*pid))
                    .clone()
                    .map(|process| (key, process))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsof_uses_both_addresses_and_refuses_ambiguous_owners() {
        let v4 = SocketKey::Tcp {
            local: "127.0.0.1:59478".parse().unwrap(),
            peer: "127.0.0.1:7898".parse().unwrap(),
        };
        let v6 = SocketKey::Tcp {
            local: "[::1]:59478".parse().unwrap(),
            peer: "[::1]:7898".parse().unwrap(),
        };
        let output = concat!(
            "p100\0cnc\0\nf1\0n127.0.0.1:59478->127.0.0.1:7898\0\n",
            "p200\0cPython\0\nf2\0n[::1]:59478->[::1]:7898\0\n",
            "p300\0cOther\0\nf3\0n127.0.0.1:59478->127.0.0.1:8000\0\n",
        );
        let wanted = HashSet::from([v4, v6]);
        let resolved = parse_lsof_output(output, &wanted);
        assert_eq!(resolved[&v4].name, "nc");
        assert_eq!(resolved[&v6].name, "Python");
        let ambiguous =
            format!("{output}p400\0cShared\0\nf1\0n127.0.0.1:59478->127.0.0.1:7898\0\n");
        assert!(!parse_lsof_output(&ambiguous, &wanted).contains_key(&v4));
    }

    #[test]
    fn process_lookup_does_not_reuse_a_result_for_a_new_connection() {
        let socket = SocketKey::Tcp {
            local: "127.0.0.1:59478".parse().unwrap(),
            peer: "127.0.0.1:7898".parse().unwrap(),
        };
        let old = ConnectionSocket {
            id: "old".into(),
            socket,
        };
        let new = ConnectionSocket {
            id: "new".into(),
            socket,
        };
        let (send, receive) = mpsc::sync_channel(1);
        send.send(HashMap::from([(
            old,
            Process {
                name: "old app".into(),
                path: String::new(),
            },
        )]))
        .unwrap();
        let mut lookup = ProcessLookup {
            pending: Some(receive),
            last_scan: Some(Instant::now()),
            ..ProcessLookup::default()
        };
        assert!(lookup.update(HashSet::from([new])).is_empty());
    }
}
