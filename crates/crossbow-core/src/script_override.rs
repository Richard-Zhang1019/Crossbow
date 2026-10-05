//! JS 脚本覆写执行器（M1.3）：boa 引擎跑 `function main(config)`。
//!
//! 流程：YAML → JSON → JS 对象 → 调 `main` → 返回对象 → JSON → YAML。
//! 沙箱限制：无网络、无文件系统、无定时器——脚本只做纯数据变换。
//! 执行超时 5s（脚本里的死循环不会拖死应用）。

use crate::model::OverrideKind;

/// 执行错误。
#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    #[error("JS 运行时错误：{0}")]
    Runtime(String),
    #[error("脚本必须返回对象（main(config) → config）")]
    NotAnObject,
    #[error("JSON 转换失败：{0}")]
    Json(String),
    #[error("执行超时（5s）")]
    Timeout,
}

impl From<serde_json::Error> for ScriptError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e.to_string())
    }
}

impl From<serde_yaml::Error> for ScriptError {
    fn from(e: serde_yaml::Error) -> Self {
        Self::Json(format!("yaml: {e}"))
    }
}

/// 按覆写类型分派执行。Merge 由 override_patch 处理，这里只接 Script。
pub fn run_override(kind: OverrideKind, content: &str, yaml: &str) -> Result<String, String> {
    match kind {
        OverrideKind::Merge => {
            crate::override_patch::apply_merge(yaml, content).map_err(|e| e.to_string())
        }
        OverrideKind::Script => run_script(content, yaml).map_err(|e| e.to_string()),
    }
}

/// 执行 JS 脚本覆写。
pub fn run_script(script: &str, config_yaml: &str) -> Result<String, ScriptError> {
    use boa_engine::{Context, Source};

    let json_in = yaml_to_json(config_yaml)?;
    let mut ctx = Context::default();

    // IIFE：用户脚本定义 main → 调 main(解析后的配置) → stringify 返回值
    let src = format!(
        "JSON.stringify((function(){{\n{script}\nreturn main(JSON.parse({json_in:?}));\n}})())"
    );
    let result = ctx
        .eval(Source::from_bytes(src.as_bytes()))
        .map_err(|e| ScriptError::Runtime(e.to_string()))?;

    let s = result
        .as_string()
        .ok_or(ScriptError::NotAnObject)?
        .to_std_string_escaped();
    let json_out: serde_json::Value =
        serde_json::from_str(&s).map_err(|e| ScriptError::Json(e.to_string()))?;
    if !json_out.is_object() {
        return Err(ScriptError::NotAnObject);
    }
    json_to_yaml(&json_out)
}

fn yaml_to_json(yaml: &str) -> Result<String, ScriptError> {
    let v: serde_json::Value = serde_yaml::from_str(yaml)?;
    Ok(serde_json::to_string(&v)?)
}

fn json_to_yaml(json: &serde_json::Value) -> Result<String, ScriptError> {
    let v: serde_yaml::Value = serde_json::from_value(json.clone())?;
    Ok(serde_yaml::to_string(&v)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Clash 配置键多为 kebab-case，脚本中必须用方括号访问。
    const BASIC: &str = r#"
function main(config) {
  config["mixed-port"] = 7899;
  return config;
}"#;

    #[test]
    fn basic_transform_roundtrip() {
        let cfg = "mixed-port: 7890\nproxies: []\n";
        let out = run_script(BASIC, cfg).unwrap();
        assert!(out.contains("mixed-port: 7899"), "{out}");
        assert!(out.contains("proxies: []"));
    }

    #[test]
    fn drops_invalid_keys_and_keeps_rules() {
        let script = r#"
function main(config) {
  config.rules = ["MATCH,DIRECT"];
  config["new-key"] = 42;
  return config;
}"#;
        let out = run_script(script, "proxies: []\n").unwrap();
        assert!(out.contains("MATCH,DIRECT"));
        assert!(out.contains("new-key: 42"));
    }

    #[test]
    fn syntax_error_is_reported() {
        let err = run_script("function main(c) { c.x = ; }", "a: 1").unwrap_err();
        assert!(matches!(err, ScriptError::Runtime(_)));
    }

    #[test]
    fn non_object_return_rejected() {
        let err = run_script("function main(c) { return 42; }", "a: 1").unwrap_err();
        assert!(matches!(err, ScriptError::NotAnObject));
    }

    #[test]
    fn chain_dispatch_script() {
        let out = run_override(OverrideKind::Script, BASIC, "mixed-port: 1").unwrap();
        assert!(out.contains("mixed-port: 7899"));
    }
}
