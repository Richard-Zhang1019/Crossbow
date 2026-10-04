//! 运行时配置注入：渲染后的订阅配置 + 壳层运行时参数 → 交给内核的最终文件。
//!
//! 运行时参数（端口/控制器/日志级别）始终覆盖订阅内容，避免机场配置里的
//! `port`/`external-controller` 干扰本地控制。控制器目前走 127.0.0.1 TCP +
//! 随机 secret；uds/pipe 收紧在 M1 与内核版本锁定一起做（DESIGN §3.4）。

use serde_yaml::{Mapping, Value};

use crate::override_patch::apply_merge;

/// 壳层决定并传给内核的运行时参数。
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// HTTP + SOCKS5 混合监听端口。
    pub mixed_port: u16,
    /// 是否允许局域网（false 时绑定 127.0.0.1）。
    pub allow_lan: bool,
    /// External Controller 监听端口（仅 127.0.0.1）。
    pub controller_port: u16,
    /// External Controller 访问密钥。
    pub controller_secret: String,
    /// 内核日志级别：debug / info / warning / error / silent。
    pub log_level: String,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            mixed_port: 7897,
            allow_lan: false,
            controller_port: 0, // 0 = 由调用方分配空闲端口
            controller_secret: String::new(),
            log_level: "info".into(),
        }
    }
}

/// 注入运行时参数，返回最终 YAML 字符串。`controller_port == 0` 时跳过控制器注入。
pub fn apply_runtime(base_yaml: &str, rt: &RuntimeConfig) -> Result<String, serde_yaml::Error> {
    let mut patch = Mapping::new();
    patch.insert(Value::from("mixed-port"), Value::from(rt.mixed_port));
    patch.insert(Value::from("allow-lan"), Value::from(rt.allow_lan));
    patch.insert(
        Value::from("bind-address"),
        Value::from(if rt.allow_lan { "*" } else { "127.0.0.1" }),
    );
    patch.insert(Value::from("mode"), Value::from("rule"));
    patch.insert(Value::from("log-level"), Value::from(rt.log_level.as_str()));
    if rt.controller_port != 0 {
        patch.insert(
            Value::from("external-controller"),
            Value::from(format!("127.0.0.1:{}", rt.controller_port)),
        );
        patch.insert(
            Value::from("secret"),
            Value::from(rt.controller_secret.as_str()),
        );
    }
    let patch_yaml = serde_yaml::to_string(&Value::Mapping(patch))?;
    apply_merge(base_yaml, &patch_yaml)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_overrides_subscription_ports() {
        let base = "mixed-port: 12345\nproxies: []\n";
        let out = apply_runtime(
            base,
            &RuntimeConfig {
                mixed_port: 7897,
                allow_lan: false,
                controller_port: 19090,
                controller_secret: "s3cret".into(),
                log_level: "info".into(),
            },
        )
        .unwrap();
        assert!(out.contains("mixed-port: 7897"));
        assert!(out.contains("bind-address: 127.0.0.1"));
        assert!(out.contains("127.0.0.1:19090"));
        assert!(out.contains("s3cret"));
        assert!(out.contains("proxies: []"), "订阅内容必须保留");
    }

    #[test]
    fn allow_lan_binds_all() {
        let out = apply_runtime(
            "proxies: []",
            &RuntimeConfig {
                allow_lan: true,
                ..RuntimeConfig::default()
            },
        )
        .unwrap();
        assert!(out.contains("bind-address: '*'"));
        assert!(out.contains("allow-lan: true"));
    }

    #[test]
    fn controller_port_zero_skips_controller() {
        let out = apply_runtime("proxies: []", &RuntimeConfig::default()).unwrap();
        assert!(!out.contains("external-controller"));
        assert!(!out.contains("secret:"));
    }
}
