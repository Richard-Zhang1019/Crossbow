//! RFC 1928/1929 on an authenticated loopback worker. No environment proxy/DNS path.
use crate::node_probes::{Budget, Check};
use std::{
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpStream, UdpSocket},
    time::{Duration, Instant},
};

fn read_exact(stream: &mut TcpStream, buf: &mut [u8], budget: &Budget) -> Result<(), String> {
    let mut offset = 0;
    while offset < buf.len() {
        budget.check()?;
        match stream.read(&mut buf[offset..]) {
            Ok(0) => return Err("测试隧道提前关闭".into()),
            Ok(n) => offset += n,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(_) => return Err("无法读取测试隧道".into()),
        }
    }
    Ok(())
}
fn write_all(stream: &mut TcpStream, buf: &[u8], budget: &Budget) -> Result<(), String> {
    let mut offset = 0;
    while offset < buf.len() {
        budget.check()?;
        match stream.write(&buf[offset..]) {
            Ok(0) => return Err("测试隧道已关闭".into()),
            Ok(n) => offset += n,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(_) => return Err("无法写入测试隧道".into()),
        }
    }
    Ok(())
}
pub fn authenticate(port: u16, password: &str, budget: &Budget) -> Result<TcpStream, String> {
    budget.check()?;
    let mut stream = TcpStream::connect_timeout(
        &SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        Duration::from_millis(100),
    )
    .map_err(|_| "无法连接隔离测试内核")?;
    stream
        .set_nonblocking(true)
        .map_err(|_| "无法设置测试套接字")?;
    write_all(&mut stream, &[5, 1, 2], budget)?;
    let mut response = [0; 2];
    read_exact(&mut stream, &mut response, budget)?;
    if response != [5, 2] {
        return Err("测试内核没有启用预期认证".into());
    }
    let mut auth = vec![1, 5];
    auth.extend_from_slice(b"probe");
    auth.push(password.len() as u8);
    auth.extend_from_slice(password.as_bytes());
    write_all(&mut stream, &auth, budget)?;
    read_exact(&mut stream, &mut response, budget)?;
    if response != [1, 0] {
        return Err("测试内核认证失败".into());
    }
    Ok(stream)
}
fn reply(stream: &mut TcpStream, budget: &Budget) -> Result<SocketAddr, String> {
    let mut header = [0; 4];
    read_exact(stream, &mut header, budget)?;
    if header[0] != 5 || header[2] != 0 || header[1] != 0 {
        return Err(if header[1] == 7 {
            "测试内核不支持该传输"
        } else {
            "目标隧道连接未成功"
        }
        .into());
    }
    let ip = match header[3] {
        1 => {
            let mut b = [0; 4];
            read_exact(stream, &mut b, budget)?;
            std::net::IpAddr::V4(b.into())
        }
        4 => {
            let mut b = [0; 16];
            read_exact(stream, &mut b, budget)?;
            std::net::IpAddr::V6(b.into())
        }
        _ => return Err("测试内核返回了不支持的绑定地址".into()),
    };
    let mut port = [0; 2];
    read_exact(stream, &mut port, budget)?;
    Ok(SocketAddr::new(ip, u16::from_be_bytes(port)))
}
pub fn tcp(port: u16, password: &str, budget: &Budget) -> Check {
    let start = Instant::now();
    let result = (|| {
        let mut stream = authenticate(port, password, budget)?;
        let host = b"speed.cloudflare.com";
        let mut request = vec![5, 1, 0, 3, host.len() as u8];
        request.extend_from_slice(host);
        request.extend_from_slice(&443u16.to_be_bytes());
        write_all(&mut stream, &request, budget)?;
        reply(&mut stream, budget)?;
        Ok::<_, String>(())
    })();
    if result.is_ok() {
        Check::ok(start.elapsed(), 0)
    } else {
        Check::unknown("tunnel", start.elapsed(), 0)
    }
}
const QUESTION: &[u8] = b"\x07example\x03com\x00\x00\x01\x00\x01";
fn query(id: [u8; 2]) -> Vec<u8> {
    let mut q = vec![id[0], id[1], 1, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    q.extend_from_slice(QUESTION);
    q
}
fn valid_dns(bytes: &[u8], id: [u8; 2]) -> bool {
    bytes.len() >= 12 + QUESTION.len()
        && bytes[..2] == id
        && bytes[2] & 0x80 != 0
        && bytes[2] & 0x78 == 0
        && bytes[4..6] == [0, 1]
        && &bytes[12..12 + QUESTION.len()] == QUESTION
        && matches!(bytes[3] & 15, 0 | 3)
}
fn payload(packet: &[u8]) -> Option<&[u8]> {
    if packet.len() < 10
        || packet[..4] != [0, 0, 0, 1]
        || packet[4..8] != [1, 1, 1, 1]
        || packet[8..10] != [0, 53]
    {
        return None;
    }
    Some(&packet[10..])
}
pub fn udp(port: u16, password: &str, budget: &Budget) -> Check {
    let start = Instant::now();
    let mut sent = 0;
    let run: Result<(), String> = (|| {
        let mut control = authenticate(port, password, budget)?;
        write_all(&mut control, &[5, 3, 0, 1, 0, 0, 0, 0, 0, 0], budget)?;
        let mut relay = reply(&mut control, budget)?;
        if relay.ip().is_unspecified() {
            relay.set_ip(Ipv4Addr::LOCALHOST.into());
        }
        if !relay.ip().is_loopback() || relay.port() == 0 {
            return Err("测试 UDP 中继不是本机地址".into());
        }
        let socket = UdpSocket::bind("127.0.0.1:0").map_err(|_| "无法创建 UDP 测试套接字")?;
        socket.connect(relay).map_err(|_| "无法连接本机 UDP 中继")?;
        socket
            .set_nonblocking(true)
            .map_err(|_| "无法设置 UDP 测试套接字")?;
        for _ in 0..3 {
            budget.check()?;
            let nonce = uuid::Uuid::new_v4();
            let id = [nonce.as_bytes()[0], nonce.as_bytes()[1]];
            let mut packet = vec![0, 0, 0, 1, 1, 1, 1, 1, 0, 53];
            packet.extend_from_slice(&query(id));
            socket.send(&packet).map_err(|_| "无法发送 UDP 测试")?;
            sent += 1;
            let attempt = budget.child(Duration::from_millis(1200));
            let mut buf = [0; 1500];
            let mut received = 0;
            while attempt.check().is_ok() {
                match socket.recv(&mut buf) {
                    Ok(n) => {
                        received += 1;
                        if payload(&buf[..n]).is_some_and(|p| valid_dns(p, id)) {
                            return Ok(());
                        }
                        if received >= 16 {
                            break;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(_) => break,
                }
            }
        }
        Err("未收到有效 UDP DNS 响应".into())
    })();
    if run.is_ok() {
        Check::ok(start.elapsed(), sent)
    } else {
        Check::unknown("udp_unverified", start.elapsed(), sent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn udp_requires_original_endpoint_nonce_question_and_non_fragmented_packet() {
        let id = [1, 2];
        let mut answer = query(id);
        answer[2] = 0x81;
        let mut packet = vec![0, 0, 0, 1, 1, 1, 1, 1, 0, 53];
        packet.extend_from_slice(&answer);
        assert!(payload(&packet).is_some_and(|p| valid_dns(p, id)));
        assert!(!valid_dns(&answer, [3, 4]));
        packet[2] = 1;
        assert!(payload(&packet).is_none());
        packet[2] = 0;
        packet[4] = 8;
        assert!(payload(&packet).is_none());
        answer[3] = 2;
        assert!(!valid_dns(&answer, id));
        answer[3] = 3;
        assert!(valid_dns(&answer, id));
        answer[12] = 0;
        assert!(!valid_dns(&answer, id));
    }
}

#[cfg(test)]
mod native_tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };
    #[test]
    #[ignore = "requires explicit isolated native core fixture"]
    fn native_http_success_udp_blackhole_is_unverified() {
        let binary = std::env::var("CB_PROBE_CORE").expect("CB_PROBE_CORE");
        let engine = if std::env::var("CB_PROBE_ENGINE").as_deref() == Ok("singbox") {
            crossbow_core::Engine::SingBox
        } else {
            crossbow_core::Engine::Mihomo
        };
        let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        tcp.set_nonblocking(true).unwrap();
        let upstream = tcp.local_addr().unwrap().port();
        let blackhole = UdpSocket::bind("127.0.0.1:0").unwrap();
        blackhole.set_nonblocking(true).unwrap();
        let udp_port = blackhole.local_addr().unwrap().port();
        let done = Arc::new(AtomicBool::new(false));
        let end = done.clone();
        let packets = Arc::new(AtomicUsize::new(0));
        let count = packets.clone();
        let server = std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(15);
            let mut readers = vec![];
            while !end.load(Ordering::SeqCst) && Instant::now() < until {
                let mut packet = [0; 1500];
                if blackhole.recv(&mut packet).is_ok() {
                    count.fetch_add(1, Ordering::SeqCst);
                }
                if let Ok((mut stream, _)) = tcp.accept() {
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_millis(200)))
                        .unwrap();
                    let stop = end.clone();
                    readers.push(std::thread::spawn(move || {
                        let mut greeting=[0;2];if stream.read_exact(&mut greeting).is_err(){return;}let mut methods=vec![0;greeting[1] as usize];if stream.read_exact(&mut methods).is_err(){return;}stream.write_all(&[5,0]).unwrap();
                        let mut header=[0;4];if stream.read_exact(&mut header).is_err(){return;}
                        let len=match header[3]{1=>4,4=>16,3=>{let mut n=[0];stream.read_exact(&mut n).unwrap();n[0] as usize},_=>return};
                        let mut target=vec![0;len+2];if stream.read_exact(&mut target).is_err(){return;}
                        let bound=if header[1]==3 {udp_port}else{1};let mut reply=vec![5,0,0,1,127,0,0,1];reply.extend_from_slice(&bound.to_be_bytes());stream.write_all(&reply).unwrap();
                        if header[1]==3 {while !stop.load(Ordering::SeqCst){std::thread::sleep(Duration::from_millis(10));}}
                        else {let mut request=[0;4096];let _=stream.read(&mut request);let _=stream.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");}
                    }));
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            end.store(true, Ordering::SeqCst);
            for reader in readers {
                reader.join().unwrap();
            }
        });
        let source = if engine == crossbow_core::Engine::Mihomo {
            serde_json::json!({"proxies":[{"name":"fixture,udp","type":"socks5","server":"127.0.0.1","port":upstream,"udp":true}]})
        } else {
            serde_json::json!({"outbounds":[{"tag":"fixture,udp","type":"socks","server":"127.0.0.1","server_port":upstream,"version":"5"}]})
        };
        let budget = Budget::new(Arc::new(|| true));
        let worker = crate::probe_worker::Worker::start(
            std::path::Path::new(&binary),
            &source.to_string(),
            engine,
            "fixture,udp",
            &budget,
        )
        .unwrap();
        let agent = ureq::AgentBuilder::new()
            .proxy(
                ureq::Proxy::new(format!(
                    "socks5://probe:{}@127.0.0.1:{}",
                    worker.password, worker.port
                ))
                .unwrap(),
            )
            .try_proxy_from_env(false)
            .timeout(Duration::from_secs(2))
            .build();
        assert_eq!(
            agent
                .get("http://127.0.0.1:9/fixture")
                .call()
                .unwrap()
                .status(),
            204
        );
        let result = udp(
            worker.port,
            &worker.password,
            &budget.child(Duration::from_secs(4)),
        );
        drop(worker);
        done.store(true, Ordering::SeqCst);
        server.join().unwrap();
        assert_eq!(result.class, "udp_unverified");
        assert_eq!(result.packets, 3);
        assert!(packets.load(Ordering::SeqCst) > 0);
        assert!(packets.load(Ordering::SeqCst) <= 3);
        println!("native evidence: engine={engine:?}, HTTP=204, UDP=unverified, datagrams={}, no WAN traffic",packets.load(Ordering::SeqCst));
    }
}
