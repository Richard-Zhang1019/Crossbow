//! JS 脚本覆写执行器（M1.3）：boa 引擎跑 `function main(config)`。
//!
//! 流程：YAML → JSON → JS 对象 → 调 `main` → 返回对象 → JSON → YAML。
//! 沙箱限制：无网络、无文件系统、无定时器——脚本只做纯数据变换。
//!
//! 执行预算（#592）：脚本默认在**独立工作进程**（本二进制以
//! `--script-worker` 参数重入自身）中执行——同步死循环无法在线程层
//! 强杀，工作进程由调用方墙钟 5s 到点 `kill`，执行资源真实释放；
//! 工作进程内另有 Boa 循环/递归限制与输出上限作为第一道预算
//! （死循环毫秒级自终止，即使父进程未及 kill 也不残留耗 CPU）。
//! 校验、启动、更新、覆写编辑共用 `override_patch::run_override_kind`
//! 单一入口，预算覆盖所有路径。预算耗尽与运行时错误分类上报；
//! 超限保留旧配置（调用方既有语义），注释承诺自本版起真实生效。

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use crate::model::OverrideKind;

/// 工作进程墙钟截止：到点强制 kill，脚本资源随之释放。
pub(crate) const WORKER_TIMEOUT: Duration = Duration::from_secs(5);
/// 工作进程模式参数：桌面入口据此分派，不进入正常应用启动。
pub const WORKER_ARG: &str = "--script-worker";
/// Boa 循环迭代上限：死循环毫秒级触发，正常配置变换远用不到。
const LOOP_LIMIT: u64 = 1_000_000;
/// 递归深度上限：深递归在此终止，不触原生栈溢出。
const RECURSION_LIMIT: usize = 256;
/// 输出上限：stringify 结果超限按预算耗尽拒绝（大输出拦截在出口）。
const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
/// 子进程轮询间隔。
const WAIT_TICK: Duration = Duration::from_millis(10);

/// 执行错误。
#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    #[error("JS 运行时错误：{0}")]
    Runtime(String),
    #[error("脚本必须返回对象（main(config) → config）")]
    NotAnObject,
    #[error("JSON 转换失败：{0}")]
    Json(String),
    #[error("执行超时（5s），脚本已强制终止")]
    Timeout,
    #[error("脚本超出执行预算（循环/递归/输出上限），已终止")]
    Budget,
}

impl ScriptError {
    /// 工作进程结果行的错误分类（与前端分类上报对应）。
    fn kind(&self) -> &'static str {
        match self {
            Self::Budget => "budget",
            Self::Timeout => "timeout",
            Self::NotAnObject => "notobject",
            Self::Json(_) => "json",
            Self::Runtime(_) => "runtime",
        }
    }
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

/// 经独立工作进程执行脚本覆写（#592）：墙钟 5s 强制 kill。
/// 工作进程无法启动（异常环境）时回退进程内预算执行——Boa 循环限制
/// 保证死循环也不挂住调用方。
pub(crate) fn run_script_via_worker(
    script: &str,
    config_yaml: &str,
) -> Result<String, ScriptError> {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        // 无工作进程环境（单测等）：进程内预算执行
        Err(_) => return run_script(script, config_yaml),
    };
    let mut child = match std::process::Command::new(exe)
        .arg(WORKER_ARG)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        // spawn 失败回退进程内执行：Boa 预算保证死循环不挂住调用方
        Err(_) => return run_script(script, config_yaml),
    };
    let payload = serde_json::json!({ "script": script, "config": config_yaml }).to_string();
    if let Some(mut stdin) = child.stdin.take() {
        std::thread::spawn(move || {
            let _ = stdin.write_all(payload.as_bytes());
        });
    }
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| ScriptError::Runtime("工作进程缺少输出管道".into()))?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        out
    });
    let deadline = Instant::now() + WORKER_TIMEOUT;
    let status = wait_with_deadline(&mut child, deadline);
    let out = reader.join().unwrap_or_default();
    if status.is_none() {
        return Err(ScriptError::Timeout);
    }
    if out.trim().is_empty() {
        // 工作进程无输出（异常环境，如测试二进制不识别 --script-worker）：
        // 进程内预算执行兜底，语义不缺位（#592）
        return run_script(script, config_yaml);
    }
    parse_worker_output(&out)
}

/// 有界等待子进程退出；超时 kill（真实终止执行）并等待收尸。
fn wait_with_deadline(
    child: &mut std::process::Child,
    deadline: Instant,
) -> Option<std::process::ExitStatus> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(WAIT_TICK),
            Err(_) => return None,
        }
    }
}

/// 解析工作进程的单行 JSON 结果。
fn parse_worker_output(out: &str) -> Result<String, ScriptError> {
    let line = out.trim();
    if line.is_empty() {
        return Err(ScriptError::Runtime("工作进程无输出".into()));
    }
    let value: serde_json::Value = serde_json::from_str(line)
        .map_err(|e| ScriptError::Json(format!("工作进程输出解析失败：{e}")))?;
    if value.get("ok").and_then(|x| x.as_bool()).unwrap_or(false) {
        let result = value
            .get("result")
            .and_then(|x| x.as_str())
            .ok_or_else(|| ScriptError::Json("工作进程结果缺少 result 字段".into()))?;
        Ok(result.to_string())
    } else {
        let kind = value
            .get("kind")
            .and_then(|x| x.as_str())
            .unwrap_or("runtime");
        let message = value
            .get("message")
            .and_then(|x| x.as_str())
            .unwrap_or("未知错误")
            .to_string();
        Err(match kind {
            "budget" => ScriptError::Budget,
            "timeout" => ScriptError::Timeout,
            "notobject" => ScriptError::NotAnObject,
            "json" => ScriptError::Json(message),
            _ => ScriptError::Runtime(message),
        })
    }
}

/// 工作进程模式入口：stdin 读 payload，stdout 写单行 JSON 结果。
/// 桌面入口在正常启动前分派（`--script-worker`），本函数不返回。
pub fn script_worker_entry() -> ! {
    let mut payload = String::new();
    let _ = std::io::stdin().read_to_string(&mut payload);
    println!("{}", worker_eval(&payload));
    std::process::exit(0);
}

/// 工作进程求值核心：payload `{script, config}` → 结果 JSON 行。
/// 任何输入问题都以 `{ok:false, kind, message}` 返回，绝不 panic。
fn worker_eval(payload: &str) -> String {
    let value = match serde_json::from_str::<serde_json::Value>(payload) {
        Ok(value) => value,
        Err(e) => return error_line("json", &format!("工作进程输入解析失败：{e}")),
    };
    let (script, config) = match (
        value.get("script").and_then(|x| x.as_str()),
        value.get("config").and_then(|x| x.as_str()),
    ) {
        (Some(script), Some(config)) => (script.to_string(), config.to_string()),
        _ => return error_line("json", "工作进程输入缺少 script/config 字段"),
    };
    match run_script(&script, &config) {
        Ok(result) => serde_json::json!({ "ok": true, "result": result }).to_string(),
        Err(error) => error_line(error.kind(), &error.to_string()),
    }
}

fn error_line(kind: &str, message: &str) -> String {
    serde_json::json!({ "ok": false, "kind": kind, "message": message }).to_string()
}

/// 执行 JS 脚本覆写（进程内预算执行）：Boa 循环/递归限制 + 输出上限。
pub fn run_script(script: &str, config_yaml: &str) -> Result<String, ScriptError> {
    use boa_engine::{Context, Source};

    let json_in = yaml_to_json(config_yaml)?;
    let mut ctx = Context::default();
    let limits = ctx.runtime_limits_mut();
    limits.set_loop_iteration_limit(LOOP_LIMIT);
    limits.set_recursion_limit(RECURSION_LIMIT);

    // IIFE：用户脚本定义 main → 调 main(解析后的配置) → stringify 返回值
    let src = format!(
        "JSON.stringify((function(){{\n{script}\nreturn main(JSON.parse({json_in:?}));\n}})())"
    );
    let result = ctx
        .eval(Source::from_bytes(src.as_bytes()))
        .map_err(classify)?;

    let s = result
        .as_string()
        .ok_or(ScriptError::NotAnObject)?
        .to_std_string_escaped();
    if s.len() > OUTPUT_LIMIT {
        return Err(ScriptError::Budget);
    }
    let json_out: serde_json::Value =
        serde_json::from_str(&s).map_err(|e| ScriptError::Json(e.to_string()))?;
    if !json_out.is_object() {
        return Err(ScriptError::NotAnObject);
    }
    json_to_yaml(&json_out)
}

/// Boa 预算超限分类：循环/递归/栈上限统一按预算耗尽上报，与普通
/// 运行时错误（语法错、字段错等）区分（#592 验收）。
fn classify(error: boa_engine::JsError) -> ScriptError {
    let message = error.to_string();
    if message.contains("iteration loops")
        || message.contains("recursive calls")
        || message.contains("maximum stack size")
    {
        ScriptError::Budget
    } else {
        ScriptError::Runtime(message)
    }
}

fn yaml_to_json(yaml: &str) -> Result<String, ScriptError> {
    let v: serde_yaml::Value = serde_yaml::from_str(yaml)?;
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

    /// #592：死循环在 Boa 循环预算内毫秒级终止并按预算耗尽分类。
    #[test]
    fn infinite_loop_hits_budget_without_hanging() {
        let started = Instant::now();
        let err = run_script("function main(c) { while (true) {} return c; }", "a: 1").unwrap_err();
        assert!(matches!(err, ScriptError::Budget), "{err:?}");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "预算必须快速终止"
        );
    }

    /// #592：深递归按预算耗尽终止。
    #[test]
    fn deep_recursion_hits_budget() {
        let err = run_script(
            "function main(c) { function f() { return f(); } return f(); }",
            "a: 1",
        )
        .unwrap_err();
        assert!(matches!(err, ScriptError::Budget), "{err:?}");
    }

    /// #592：超限输出按预算耗尽拒绝（拦截在出口，不进入下游）。
    #[test]
    fn oversized_output_hits_budget() {
        let script = r#"
function main(config) {
  return { data: "x".repeat(9000000) };
}"#;
        let err = run_script(script, "a: 1").unwrap_err();
        assert!(matches!(err, ScriptError::Budget), "{err:?}");
    }

    /// #592：工作进程求值核心的正常/错误两条路。
    #[test]
    fn worker_eval_roundtrip_and_error() {
        let payload = serde_json::json!({ "script": BASIC, "config": "mixed-port: 1" }).to_string();
        let line = worker_eval(&payload);
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["ok"], true);
        assert!(value["result"]
            .as_str()
            .unwrap()
            .contains("mixed-port: 7899"));

        let bad =
            serde_json::json!({ "script": "function main(c) { while (true) {} }", "config": "" })
                .to_string();
        let line = worker_eval(&bad);
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["ok"], false);
        assert_eq!(value["kind"], "budget");
    }

    /// #592：墙钟到点强制 kill 卡死子进程的机制（外部硬截止不挂 CI）。
    #[test]
    #[cfg(unix)]
    fn wait_with_deadline_kills_stuck_child() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_millis(200);
        assert!(wait_with_deadline(&mut child, deadline).is_none());
        std::thread::sleep(Duration::from_millis(120));
        // 子进程已被 kill：再次 try_wait 确认已退出，无残留
        assert!(child.try_wait().unwrap().is_some());
    }

    /// #592：工作进程输出行的解析映射（ok 与分类错误）。
    #[test]
    fn parse_worker_output_maps_kinds() {
        let ok = parse_worker_output(r#"{"ok":true,"result":"mixed-port: 7899"}"#).unwrap();
        assert_eq!(ok, "mixed-port: 7899");
        let budget =
            parse_worker_output(r#"{"ok":false,"kind":"budget","message":"x"}"#).unwrap_err();
        assert!(matches!(budget, ScriptError::Budget));
        let timeout =
            parse_worker_output(r#"{"ok":false,"kind":"timeout","message":"x"}"#).unwrap_err();
        assert!(matches!(timeout, ScriptError::Timeout));
        let empty = parse_worker_output("").unwrap_err();
        assert!(matches!(empty, ScriptError::Runtime(_)));
    }
}
