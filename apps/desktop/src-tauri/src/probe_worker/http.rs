//! HTTPS verification/transfer reuse ureq's certificate, framing and deadline handling.
use crate::node_probes::{Budget, Check, Transfer};
use std::{io::Read, time::Instant};
pub const DOWNLOAD_BYTES: u64 = 4 * 1024 * 1024;
pub const UPLOAD_BYTES: u64 = 1024 * 1024;
fn agent(port: u16, password: &str, budget: &Budget) -> Result<ureq::Agent, String> {
    budget.check()?;
    let proxy = ureq::Proxy::new(format!("socks5://probe:{password}@127.0.0.1:{port}"))
        .map_err(|_| "无法建立测试代理")?;
    Ok(ureq::AgentBuilder::new()
        .proxy(proxy)
        .try_proxy_from_env(false)
        .redirects(0)
        // ureq applies timeout_connect to the full SOCKS handshake, including
        // the chosen outbound's remote dial. Use the phase's overall deadline
        // for both CONNECT and HTTP/TLS instead of a loopback-only 500ms cap.
        .timeout(budget.remaining()?)
        .build())
}
fn encoding(response: &ureq::Response) -> bool {
    response
        .header("content-encoding")
        .is_none_or(|v| v.eq_ignore_ascii_case("identity"))
}
pub fn https(port: u16, password: &str, budget: &Budget) -> Check {
    let start = Instant::now();
    let result = (|| {
        let response = agent(port, password, budget)?
            .get("https://speed.cloudflare.com/__down?bytes=0")
            .set("Accept-Encoding", "identity")
            .call()
            .map_err(|_| "HTTPS 请求未完成")?;
        budget.check()?;
        if response.status() != 200 || !encoding(&response) {
            return Err("测试目标响应异常".into());
        }
        let mut extra = [0; 1];
        if response
            .into_reader()
            .read(&mut extra)
            .map_err(|_| "HTTPS 响应未完成")?
            != 0
        {
            return Err("测试目标返回了非预期响应".into());
        }
        Ok::<_, String>(())
    })();
    if result.is_ok() {
        Check::ok(start.elapsed(), 1)
    } else {
        Check::unknown("https_unverified", start.elapsed(), 1)
    }
}
fn read_download(reader: &mut dyn Read, budget: &Budget) -> (u64, bool) {
    let mut bytes = 0;
    let mut buffer = [0; 16 * 1024];
    while bytes < DOWNLOAD_BYTES {
        if budget.check().is_err() {
            return (bytes, false);
        }
        let size = buffer.len().min((DOWNLOAD_BYTES - bytes) as usize);
        match reader.read(&mut buffer[..size]) {
            Ok(0) | Err(_) => return (bytes, false),
            Ok(n) => bytes += n as u64,
        }
    }
    (bytes, true)
}
pub fn download(port: u16, password: &str, budget: &Budget) -> Transfer {
    let start = Instant::now();
    let mut bytes = 0;
    let result = (|| {
        let response = agent(port, password, budget)?
            .get(&format!(
                "https://speed.cloudflare.com/__down?bytes={DOWNLOAD_BYTES}"
            ))
            .set("Accept-Encoding", "identity")
            .call()
            .map_err(|_| "下载目标请求未完成")?;
        if response.status() != 200 || !encoding(&response) {
            return Err("下载目标响应异常".into());
        }
        let mut reader = response.into_reader();
        let (received, complete) = read_download(reader.as_mut(), budget);
        bytes = received;
        if !complete {
            return Err("下载被中断或未完整接收".into());
        }
        // Stop at the payload budget even when the server sends more.
        Ok::<_, String>(())
    })();
    Transfer::new(result.is_ok(), bytes, start.elapsed(), DOWNLOAD_BYTES)
}
pub fn upload(port: u16, password: &str, budget: &Budget) -> Transfer {
    let start = Instant::now();
    let mut bytes = 0;
    let result = (|| {
        let body = vec![b'0'; UPLOAD_BYTES as usize];
        let response = agent(port, password, budget)?
            .post("https://speed.cloudflare.com/__up")
            .set("Content-Type", "application/octet-stream")
            .set("Accept-Encoding", "identity")
            .send_bytes(&body)
            .map_err(|_| "上传目标请求未完成")?;
        // Count only a request acknowledged by the target; failed sends are unknown, not zero-byte success.
        if response.status() != 200 {
            return Err("上传目标未确认".into());
        }
        bytes = UPLOAD_BYTES;
        let mut reply = Vec::new();
        response
            .into_reader()
            .take(8193)
            .read_to_end(&mut reply)
            .map_err(|_| "上传确认未完成")?;
        if reply.len() > 8192 {
            return Err("上传确认超限".into());
        }
        budget.check()?;
        Ok::<_, String>(())
    })();
    Transfer::new(result.is_ok(), bytes, start.elapsed(), UPLOAD_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    // A SOCKS CONNECT reply arrives only after the selected outbound has dialed
    // the target. Delaying it models a usable, high-latency proxy without WAN IO.
    fn delayed_proxy() -> (u16, std::thread::JoinHandle<()>) {
        use std::{io::Write, net::TcpListener};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let serve = || -> std::io::Result<()> {
                let mut socket = &socket;
                let mut header = [0; 2];
                socket.read_exact(&mut header)?;
                let mut methods = vec![0; header[1] as usize];
                socket.read_exact(&mut methods)?;
                socket.write_all(&[5, 2])?;
                socket.read_exact(&mut header)?;
                let mut user = vec![0; header[1] as usize];
                socket.read_exact(&mut user)?;
                let mut size = [0];
                socket.read_exact(&mut size)?;
                let mut password = vec![0; size[0] as usize];
                socket.read_exact(&mut password)?;
                socket.write_all(&[1, 0])?;
                let mut connect = [0; 4];
                socket.read_exact(&mut connect)?;
                assert_eq!(connect, [5, 1, 0, 3]);
                socket.read_exact(&mut size)?;
                let mut target = vec![0; size[0] as usize + 2];
                socket.read_exact(&mut target)?;
                std::thread::sleep(Duration::from_millis(700));
                socket.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 80])?;
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") && request.len() < 4096 {
                    let mut byte = [0];
                    if socket.read(&mut byte)? == 0 {
                        return Ok(());
                    }
                    request.push(byte[0]);
                }
                socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            };
            // An expired request closes its socket before the delayed reply.
            let _ = serve();
        });
        (port, server)
    }
    #[test]
    fn socks_target_dial_uses_stage_budget_instead_of_half_second_cap() {
        let (port, server) = delayed_proxy();
        let budget = Budget::new(std::sync::Arc::new(|| true)).child(Duration::from_secs(3));
        let result = agent(port, "nonce", &budget)
            .unwrap()
            .get("http://probe.invalid/")
            .call();
        server.join().unwrap();
        assert_eq!(
            result
                .expect("700ms outbound dial is within the 3s phase budget")
                .status(),
            200
        );

        let (port, server) = delayed_proxy();
        let budget = Budget::new(std::sync::Arc::new(|| true)).child(Duration::from_millis(100));
        let result = agent(port, "nonce", &budget)
            .unwrap()
            .get("http://probe.invalid/")
            .call();
        server.join().unwrap();
        assert!(
            result.is_err(),
            "the phase deadline must still bound SOCKS CONNECT"
        );
    }
    #[test]
    fn download_caps_reads_and_does_not_accept_a_short_or_cancelled_body() {
        let budget = Budget::new(std::sync::Arc::new(|| true));
        let mut oversized = std::io::repeat(0);
        assert_eq!(
            read_download(&mut oversized, &budget),
            (DOWNLOAD_BYTES, true)
        );
        let mut short = std::io::Cursor::new(vec![0; 31]);
        assert_eq!(read_download(&mut short, &budget), (31, false));
        let invalid = Budget::new(std::sync::Arc::new(|| false));
        let mut untouched = std::io::Cursor::new(vec![0; 31]);
        assert_eq!(read_download(&mut untouched, &invalid), (0, false));
        assert_eq!(untouched.position(), 0);
    }
}
