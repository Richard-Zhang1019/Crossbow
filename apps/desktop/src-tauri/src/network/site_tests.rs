//! Small, allowlisted probes through Crossbow's explicit loopback HTTP proxy.
use std::time::{Duration, Instant};

#[derive(Debug, Clone, serde::Serialize)]
pub struct SiteResult {
    pub id: String,
    pub status: String,
    pub http_status: Option<u16>,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
}

fn target(id: &str) -> Option<&'static str> {
    match id {
        "youtube" => Some("https://www.youtube.com/"),
        "x" => Some("https://x.com/"),
        "google" => Some("https://www.google.com/"),
        "github" => Some("https://github.com/"),
        _ => None,
    }
}

pub fn probe(id: &str, port: u16, running: bool) -> Result<SiteResult, String> {
    let url = target(id).ok_or("Unsupported connectivity test target")?;
    if !running {
        return Ok(SiteResult {
            id: id.into(),
            status: "error".into(),
            http_status: None,
            latency_ms: None,
            error: Some("core_stopped".into()),
        });
    }
    Ok(request(id, url, port))
}

fn request(id: &str, url: &str, port: u16) -> SiteResult {
    let started = Instant::now();
    let proxy = ureq::Proxy::new(format!("http://127.0.0.1:{port}")).expect("loopback proxy URL");
    let agent = ureq::AgentBuilder::new()
        .try_proxy_from_env(false)
        .proxy(proxy)
        .timeout_connect(Duration::from_secs(3))
        .timeout(Duration::from_secs(8))
        .redirects(3)
        .build();
    let response = agent.head(url).call();
    let elapsed = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let (status, http_status, error) = match response {
        Ok(response) => ("ok", Some(response.status()), None),
        Err(ureq::Error::Status(code, _)) => (
            if matches!(code, 401 | 403 | 429) {
                "restricted"
            } else {
                "http_error"
            },
            Some(code),
            None,
        ),
        Err(ureq::Error::Transport(error)) => {
            let text = error.to_string().to_lowercase();
            let reason = if text.contains("timed out")
                || text.contains("timeout")
                || text.contains("deadline")
            {
                "timeout"
            } else if text.contains("tls") || text.contains("certificate") || text.contains("ssl") {
                "tls"
            } else {
                match error.kind() {
                    ureq::ErrorKind::Dns => "dns",
                    ureq::ErrorKind::TooManyRedirects => "redirect",
                    ureq::ErrorKind::ProxyConnect | ureq::ErrorKind::ProxyUnauthorized => "proxy",
                    ureq::ErrorKind::ConnectionFailed => "connection",
                    _ => "network",
                }
            };
            ("error", None, Some(reason.into()))
        }
    };
    SiteResult {
        id: id.into(),
        status: status.into(),
        http_status,
        latency_ms: if http_status.is_some() {
            Some(elapsed)
        } else {
            None
        },
        error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    #[test]
    fn targets_are_allowlisted_and_stopped_core_does_not_probe() {
        assert!(probe("http://localhost/private", 1, false).is_err());
        assert_eq!(
            probe("youtube", 1, false).unwrap().error.as_deref(),
            Some("core_stopped")
        );
    }
    #[test]
    fn request_uses_explicit_proxy_and_distinguishes_http_restrictions() {
        for (code, expected) in [(204, "ok"), (403, "restricted")] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                assert!(first.starts_with("HEAD http://site.invalid/ HTTP/1.1"));
                let mut line = String::new();
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                }
                write!(
                    stream,
                    "HTTP/1.1 {code} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
            });
            let result = request("fixture", "http://site.invalid/", port);
            server.join().unwrap();
            assert_eq!(result.status, expected);
            assert_eq!(result.http_status, Some(code));
            assert!(result.latency_ms.is_some());
        }
    }
}
