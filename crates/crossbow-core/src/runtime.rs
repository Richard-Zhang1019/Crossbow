//! 运行时配置注入：渲染后的订阅配置 + 壳层运行时参数 → 交给内核的最终文件。
//!
//! 运行时参数（端口/控制器/日志级别）始终覆盖订阅内容，避免机场配置里的
//! 端口/控制器设置干扰本地控制。控制器目前走 127.0.0.1 TCP + 随机 secret；
//! uds/pipe 收紧在 M1 与内核版本锁定一起做（DESIGN §3.4）。
//!
//! 两个引擎两种格式：mihomo 走 YAML 合并；sing-box 输入输出都是 JSON，
//! 控制器经 experimental.clash_api（Clash 兼容 API）注入。

use serde_json::json;
use serde_yaml::{Mapping, Value};

use crate::model::Engine;
use crate::override_patch::apply_merge;

/// 壳层决定并传给内核的运行时参数。
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// 引擎（决定注入格式与键名）。
    pub engine: Engine,
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
            engine: Engine::Mihomo,
            mixed_port: 7897,
            allow_lan: false,
            controller_port: 0, // 0 = 由调用方分配空闲端口
            controller_secret: String::new(),
            log_level: "info".into(),
        }
    }
}

/// 注入运行时参数，返回最终配置字符串。`controller_port == 0` 时跳过控制器注入。
pub fn apply_runtime(base: &str, rt: &RuntimeConfig) -> Result<String, String> {
    match rt.engine {
        Engine::SingBox => apply_runtime_singbox(base, rt),
        Engine::Mihomo => apply_runtime_mihomo(base, rt),
    }
}

fn apply_runtime_mihomo(base_yaml: &str, rt: &RuntimeConfig) -> Result<String, String> {
    let mut patch = Mapping::new();
    patch.insert(Value::from("mixed-port"), Value::from(rt.mixed_port));
    patch.insert(Value::from("allow-lan"), Value::from(rt.allow_lan));
    patch.insert(
        Value::from("bind-address"),
        Value::from(if rt.allow_lan { "*" } else { "127.0.0.1" }),
    );
    patch.insert(Value::from("mode"), Value::from("rule"));
    patch.insert(Value::from("log-level"), Value::from(rt.log_level.as_str()));
    // 节点选择持久化：内核侧记住手工选择（等价「固定节点」），重启不丢。
    let mut profile = Mapping::new();
    profile.insert(Value::from("store-selected"), Value::from(true));
    patch.insert(Value::from("profile"), Value::from(profile));
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
    let patch_yaml =
        serde_yaml::to_string(&Value::Mapping(patch)).map_err(|e| e.to_string())?;
    apply_merge(base_yaml, &patch_yaml).map_err(|e| e.to_string())
}

fn apply_runtime_singbox(base_json: &str, rt: &RuntimeConfig) -> Result<String, String> {
    let mut v: serde_json::Value =
        serde_json::from_str(base_json).map_err(|e| format!("sing-box 配置非法 JSON: {e}"))?;

    let bind = if rt.allow_lan { "::" } else { "127.0.0.1" };
    if let Some(inbounds) = v.get_mut("inbounds").and_then(serde_json::Value::as_array_mut) {
        for inb in inbounds.iter_mut() {
            if inb.get("type").and_then(serde_json::Value::as_str) == Some("mixed") {
                inb["listen"] = json!(bind);
                inb["listen_port"] = json!(rt.mixed_port);
            }
        }
    }
    if let Some(log) = v.get_mut("log").filter(|l| serde_json::Value::is_object(l)) {
        log["level"] = json!(sb_log_level(&rt.log_level));
    }
    if rt.controller_port != 0 {
        if !v.get("experimental").map(serde_json::Value::is_object).unwrap_or(false) {
            v["experimental"] = json!({});
        }
        v["experimental"]["clash_api"] = json!({
            "external_controller": format!("127.0.0.1:{}", rt.controller_port),
            "secret": rt.controller_secret,
        });
    }
    serde_json::to_string_pretty(&v).map_err(|e| e.to_string())
}

/// mihomo 日志级别 → sing-box（warning→warn，silent→disable）。
fn sb_log_level(level: &str) -> &str {
    match level {
        "warning" => "warn",
        "silent" => "disable",
        other => other,
    }
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
                ..RuntimeConfig::default()
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

    // ---- sing-box：JSON in / JSON out，clash_api 注入 ----

    fn sb_base() -> String {
        // 与 singbox_convert 输出同构的最小 JSON
        r#"{
  "log": { "level": "info", "timestamp": true },
  "dns": { "final": "proxy-dns" },
  "inbounds": [
    { "type": "mixed", "tag": "mixed-in", "listen": "127.0.0.1", "listen_port": 7897 }
  ],
  "outbounds": [{ "type": "direct", "tag": "DIRECT" }],
  "route": { "final": "DIRECT" },
  "experimental": { "cache_file": { "enabled": true } }
}"#
        .into()
    }

    #[test]
    fn singbox_runtime_stays_json_and_injects_clash_api() {
        let out = apply_runtime(
            &sb_base(),
            &RuntimeConfig {
                engine: Engine::SingBox,
                mixed_port: 7899,
                allow_lan: true,
                controller_port: 19090,
                controller_secret: "sb-secret".into(),
                log_level: "warning".into(),
            },
        )
        .unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&out).expect("sing-box 最终配置必须是合法 JSON");
        let inb = &v["inbounds"][0];
        assert_eq!(inb["listen_port"], 7899);
        assert_eq!(inb["listen"], "::");
        assert_eq!(v["log"]["level"], "warn");
        assert_eq!(v["experimental"]["clash_api"]["external_controller"], "127.0.0.1:19090");
        assert_eq!(v["experimental"]["clash_api"]["secret"], "sb-secret");
        // 已有 experimental 键不被覆盖
        assert_eq!(v["experimental"]["cache_file"]["enabled"], true);
    }

    #[test]
    fn singbox_controller_zero_skips_clash_api() {
        let out = apply_runtime(
            &sb_base(),
            &RuntimeConfig {
                engine: Engine::SingBox,
                ..RuntimeConfig::default()
            },
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(v["experimental"].get("clash_api").is_none());
        assert_eq!(v["inbounds"][0]["listen_port"], 7897);
    }

    #[test]
    fn singbox_rejects_non_json_base() {
        let err = apply_runtime(
            "mixed-port: 7897\nproxies: []\n",
            &RuntimeConfig {
                engine: Engine::SingBox,
                ..RuntimeConfig::default()
            },
        )
        .unwrap_err();
        assert!(err.contains("JSON"));
    }
}
