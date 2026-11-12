//! Privileged-core policy, independent of Win32 so CI can test it on both OSes.
use serde_yaml::Value;

pub const MAX_CONFIG: u64 = 8 * 1024 * 1024;
pub const MAX_ASSET: u64 = 64 * 1024 * 1024;
// 资产上限必须大于配置上限（编译期锁定的不变量，也防常量被独立改漂）。
const _: () = assert!(MAX_ASSET > MAX_CONFIG);

pub fn relative_path(path: &str) -> Result<String, String> {
    let path = path.replace('\\', "/");
    if path.is_empty()
        || path.contains(':')
        || path.starts_with('/')
        || path.split('/').any(|p| {
            let stem = p.split('.').next().unwrap_or("").to_ascii_uppercase();
            p.is_empty()
                || p.ends_with(['.', ' '])
                || p.chars().any(|c| c.is_control() || "<>|?*".contains(c))
                || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                || (stem.len() == 4
                    && (stem.starts_with("COM") || stem.starts_with("LPT"))
                    && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        })
    {
        return Err("provider paths must stay inside the service runtime".into());
    }
    Ok(path)
}

pub fn config(bytes: &[u8]) -> Result<(String, Vec<String>), String> {
    if bytes.len() as u64 > MAX_CONFIG {
        return Err("config too large".into());
    }
    let mut yaml: Value = serde_yaml::from_slice(bytes).map_err(|_| "invalid config YAML")?;
    let root = yaml.as_mapping_mut().ok_or("config must be a mapping")?;
    let controller = root
        .get(Value::from("external-controller"))
        .and_then(Value::as_str)
        .ok_or("missing loopback controller")?;
    let addr: std::net::SocketAddr = controller.parse().map_err(|_| "invalid controller")?;
    if !addr.ip().is_loopback() || addr.port() == 0 {
        return Err("controller must be loopback".into());
    }
    if root
        .get(Value::from("secret"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .len()
        < 16
    {
        return Err("controller secret is too short".into());
    }
    // These endpoints/UI archives must never create arbitrary SYSTEM pipes/files.
    for key in [
        "external-controller-unix",
        "external-controller-pipe",
        "external-controller-tls",
        "external-ui",
        "external-ui-url",
        "external-ui-name",
    ] {
        root.remove(Value::from(key));
    }
    let mut paths = Vec::new();
    for section in ["proxy-providers", "rule-providers"] {
        if let Some(providers) = root.get(Value::from(section)).and_then(Value::as_mapping) {
            for provider in providers.values() {
                if let Some(path) = provider.get("path") {
                    paths.push(relative_path(
                        path.as_str().ok_or("invalid provider path")?,
                    )?);
                }
            }
        }
    }
    paths.sort();
    paths.dedup();
    if paths.len() > 128 {
        return Err("too many local provider files".into());
    }
    let text = serde_yaml::to_string(&yaml).map_err(|_| "cannot serialize config")?;
    Ok((text, paths))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn base() -> &'static str {
        "external-controller: 127.0.0.1:19090\nsecret: abcdef0123456789abcdef\n"
    }
    #[test]
    fn rejects_windows_escape_paths_on_any_platform() {
        for path in [
            "",
            "../x",
            "a/../../x",
            "C:\\Windows\\x",
            "\\\\host\\share",
            "/etc/x",
            "a/.. /escape",
            "a/NUL.txt",
            "a/COM1",
            "a/file. ",
            "a\\..\\x",
            "a:x",
            "a//x",
        ] {
            assert!(relative_path(path).is_err(), "{path}");
        }
        assert_eq!(
            relative_path("rules\\test.yaml").unwrap(),
            "rules/test.yaml"
        );
    }
    #[test]
    fn blocks_unauthenticated_or_remote_controllers() {
        assert!(config(b"external-controller: 0.0.0.0:90\nsecret: abcdef0123456789").is_err());
        assert!(config(b"external-controller: 127.0.0.1:90\nsecret: x").is_err());
        assert!(config(&vec![b' '; MAX_CONFIG as usize + 1]).is_err());
    }
    #[test]
    fn removes_privileged_endpoints_and_retains_safe_providers() {
        let source = format!("{}external-controller-pipe: dangerous\nexternal-ui: C:/Windows\nrule-providers:\n  x:\n    path: rules/test.yaml\n", base());
        let (yaml, paths) = config(source.as_bytes()).unwrap();
        assert!(!yaml.contains("dangerous"));
        assert!(!yaml.contains("C:/Windows"));
        assert_eq!(paths, ["rules/test.yaml"]);
        let bad = format!("{}rule-providers:\n  x:\n    path: ../escape\n", base());
        assert!(config(bad.as_bytes()).is_err());
        let ws = format!(
            "{}proxies:\n  - name: test\n    type: vmess\n    ws-opts:\n      path: /websocket\n",
            base()
        );
        assert!(config(ws.as_bytes()).is_ok());
        // 资产上限(64MB) > 配置上限(8MB) 由常量定义字面保证，无需运行时断言
        // （clippy constant_assert：编译期可折叠的比较不构成有效测试）。
    }
}
