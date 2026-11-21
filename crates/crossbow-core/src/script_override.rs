//! Script overrides run only in a killable worker, including validation/render.
//! Five seconds per script, plus a shared render/validation-chain deadline.
//! Bounded request/response, Boa loop/recursion budgets,
//! and a worker-only cumulative Rust allocation budget. There is no in-process
//! fallback on worker failures. JS receives no filesystem/network/timer APIs.

use crate::model::OverrideKind;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub(crate) const WORKER_TIMEOUT: Duration = Duration::from_secs(5);
pub const WORKER_ARG: &str = "--script-worker";
const LOOP_LIMIT: u64 = 1_000_000;
const RECURSION_LIMIT: usize = 256;
const SCRIPT_LIMIT: usize = 256 * 1024;
const CONFIG_LIMIT: usize = 8 * 1024 * 1024;
const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
// JSON escapes can expand a byte to six ASCII bytes, plus protocol overhead.
const INPUT_WIRE_LIMIT: usize = (SCRIPT_LIMIT + CONFIG_LIMIT) * 6 + 4096;
const OUTPUT_WIRE_LIMIT: usize = OUTPUT_LIMIT * 6 + 4096;
const FRAME: &str = "CROSSBOW_SCRIPT_RESULT ";
const WAIT_TICK: Duration = Duration::from_millis(10);

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
    #[error("脚本超出执行预算（输入/循环/递归/分配/输出上限），已终止")]
    Budget,
    #[error("脚本工作进程失败，未在应用内重试")]
    Worker,
}
impl ScriptError {
    fn kind(&self) -> &'static str {
        match self {
            Self::Runtime(_) => "runtime",
            Self::NotAnObject => "notobject",
            Self::Json(_) => "json",
            Self::Timeout => "timeout",
            Self::Budget => "budget",
            Self::Worker => "worker",
        }
    }
}
impl From<serde_json::Error> for ScriptError {
    fn from(_: serde_json::Error) -> Self {
        Self::Json("配置 JSON 无效".into())
    }
}
impl From<serde_yaml::Error> for ScriptError {
    fn from(_: serde_yaml::Error) -> Self {
        Self::Json("配置 YAML 无效".into())
    }
}

pub fn run_override(kind: OverrideKind, content: &str, yaml: &str) -> Result<String, String> {
    match kind {
        OverrideKind::Merge => {
            crate::override_patch::apply_merge(yaml, content).map_err(|e| e.to_string())
        }
        OverrideKind::Script => run_script(content, yaml).map_err(|e| e.to_string()),
    }
}

/// All public script entry points use this fail-closed transport.
pub fn run_script(script: &str, config: &str) -> Result<String, ScriptError> {
    run_script_with_timeout(script, config, WORKER_TIMEOUT)
}

pub(crate) fn run_script_with_timeout(
    script: &str,
    config: &str,
    timeout: Duration,
) -> Result<String, ScriptError> {
    if timeout.is_zero() {
        return Err(ScriptError::Timeout);
    }
    let mut command = Command::new(std::env::current_exe().map_err(|_| ScriptError::Worker)?);
    #[cfg(not(test))]
    command.arg(WORKER_ARG);
    #[cfg(test)]
    command.args([
        "--exact",
        "script_override::tests::worker_process",
        "--ignored",
        "--nocapture",
        "--test-threads=1",
    ]);
    run_with_command(command, script, config, timeout)
}

struct Worker {
    child: Child,
    reaped: bool,
}
impl Drop for Worker {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn run_with_command(
    mut command: Command,
    script: &str,
    config: &str,
    timeout: Duration,
) -> Result<String, ScriptError> {
    if script.len() > SCRIPT_LIMIT || config.len() > CONFIG_LIMIT {
        return Err(ScriptError::Budget);
    }
    let deadline = Instant::now() + timeout;
    // Anonymous private files avoid pipe backpressure, reader/writer thread leaks
    // and blocking EOF joins. tempfile removes them on handle/process teardown.
    let mut input = tempfile::tempfile().map_err(|_| ScriptError::Worker)?;
    #[derive(serde::Serialize)]
    struct Request<'a> {
        script: &'a str,
        config: &'a str,
    }
    {
        let mut writer = BufWriter::new(&mut input);
        serde_json::to_writer(&mut writer, &Request { script, config })
            .map_err(|_| ScriptError::Worker)?;
        writer.flush().map_err(|_| ScriptError::Worker)?;
    }
    input
        .seek(SeekFrom::Start(0))
        .map_err(|_| ScriptError::Worker)?;
    let mut output = tempfile::tempfile().map_err(|_| ScriptError::Worker)?;
    command
        .stdin(Stdio::from(input))
        .stdout(Stdio::from(
            output.try_clone().map_err(|_| ScriptError::Worker)?,
        ))
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let mut worker = Worker {
        child: command.spawn().map_err(|_| ScriptError::Worker)?,
        reaped: false,
    };
    let status = loop {
        if output.metadata().map_err(|_| ScriptError::Worker)?.len() > OUTPUT_WIRE_LIMIT as u64 {
            return Err(ScriptError::Budget);
        }
        if Instant::now() >= deadline {
            return Err(ScriptError::Timeout);
        }
        match worker.child.try_wait().map_err(|_| ScriptError::Worker)? {
            Some(status) => {
                worker.reaped = true;
                break status;
            }
            None => std::thread::sleep(WAIT_TICK),
        }
    };
    if status.code() == Some(crate::worker_memory::EXIT_BUDGET) {
        return Err(ScriptError::Budget);
    }
    if !status.success() {
        return Err(ScriptError::Worker);
    }
    output
        .seek(SeekFrom::Start(0))
        .map_err(|_| ScriptError::Worker)?;
    let mut bytes = Vec::new();
    output
        .take(OUTPUT_WIRE_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ScriptError::Worker)?;
    if bytes.len() > OUTPUT_WIRE_LIMIT {
        return Err(ScriptError::Budget);
    }
    parse_worker_output(std::str::from_utf8(&bytes).map_err(|_| ScriptError::Worker)?)
}

fn parse_worker_output(out: &str) -> Result<String, ScriptError> {
    let mut frames = out.lines().filter_map(|line| line.strip_prefix(FRAME));
    let line = frames.next().ok_or(ScriptError::Worker)?;
    if frames.next().is_some() {
        return Err(ScriptError::Worker);
    }
    let v: serde_json::Value = serde_json::from_str(line).map_err(|_| ScriptError::Worker)?;
    match v.get("ok").and_then(|v| v.as_bool()) {
        Some(true) => {
            let result = v
                .get("result")
                .and_then(|v| v.as_str())
                .ok_or(ScriptError::Worker)?;
            if result.len() > OUTPUT_LIMIT {
                return Err(ScriptError::Budget);
            }
            Ok(result.to_owned())
        }
        Some(false) => Err(match v.get("kind").and_then(|v| v.as_str()) {
            Some("budget") => ScriptError::Budget,
            Some("timeout") => ScriptError::Timeout,
            Some("notobject") => ScriptError::NotAnObject,
            Some("json") => ScriptError::Json("配置转换失败".into()),
            Some("runtime") => ScriptError::Runtime("脚本运行失败（检查语法或配置字段）".into()),
            _ => ScriptError::Worker,
        }),
        None => Err(ScriptError::Worker),
    }
}

/// The host must register WorkerAllocator. No evaluator executes on host failure.
pub fn script_worker_entry() -> ! {
    let result = if !crate::worker_memory::activate() {
        error_line("worker")
    } else {
        let mut payload = String::new();
        match std::io::stdin()
            .take(INPUT_WIRE_LIMIT as u64 + 1)
            .read_to_string(&mut payload)
        {
            Ok(_) if payload.len() <= INPUT_WIRE_LIMIT => worker_eval(&payload),
            _ => error_line("budget"),
        }
    };
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    let _ = writeln!(output, "\n{FRAME}{result}");
    let _ = output.flush();
    std::process::exit(0);
}

fn worker_eval(payload: &str) -> String {
    let value = match serde_json::from_str::<serde_json::Value>(payload) {
        Ok(value) => value,
        Err(_) => return error_line("json"),
    };
    let (Some(script), Some(config)) = (
        value.get("script").and_then(|v| v.as_str()),
        value.get("config").and_then(|v| v.as_str()),
    ) else {
        return error_line("json");
    };
    if script.len() > SCRIPT_LIMIT || config.len() > CONFIG_LIMIT {
        return error_line("budget");
    }
    match evaluate(script, config) {
        Ok(result) => serde_json::json!({"ok":true,"result":result}).to_string(),
        Err(error) => error_line(error.kind()),
    }
}
fn error_line(kind: &str) -> String {
    serde_json::json!({"ok":false,"kind":kind}).to_string()
}

/// Private evaluator, called only inside the resource-guarded worker.
fn evaluate(script: &str, config_yaml: &str) -> Result<String, ScriptError> {
    use boa_engine::{Context, Source};
    let json_in = yaml_to_json(config_yaml)?;
    let mut ctx = Context::default();
    let limits = ctx.runtime_limits_mut();
    limits.set_loop_iteration_limit(LOOP_LIMIT);
    limits.set_recursion_limit(RECURSION_LIMIT);
    let src = format!(
        "JSON.stringify((function(){{\n{script}\nreturn main(JSON.parse({json_in:?}));\n}})())"
    );
    let result = ctx
        .eval(Source::from_bytes(src.as_bytes()))
        .map_err(classify)?;
    let result = result.as_string().ok_or(ScriptError::NotAnObject)?;
    if result.len() > OUTPUT_LIMIT {
        return Err(ScriptError::Budget);
    }
    let s = result.to_std_string_escaped();
    if s.len() > OUTPUT_LIMIT {
        return Err(ScriptError::Budget);
    }
    let json: serde_json::Value = serde_json::from_str(&s)?;
    if !json.is_object() {
        return Err(ScriptError::NotAnObject);
    }
    let yaml = json_to_yaml(&json)?;
    if yaml.len() > OUTPUT_LIMIT {
        return Err(ScriptError::Budget);
    }
    Ok(yaml)
}
fn classify(error: boa_engine::JsError) -> ScriptError {
    if matches!(
        error.as_engine(),
        Some(boa_engine::error::EngineError::RuntimeLimit(_))
    ) {
        ScriptError::Budget
    } else {
        ScriptError::Runtime("脚本运行失败（检查语法或配置字段）".into())
    }
}
fn yaml_to_json(yaml: &str) -> Result<String, ScriptError> {
    let value: serde_yaml::Value = serde_yaml::from_str(yaml)?;
    Ok(serde_json::to_string(&value)?)
}
fn json_to_yaml(json: &serde_json::Value) -> Result<String, ScriptError> {
    let value: serde_yaml::Value = serde_json::from_value(json.clone())?;
    Ok(serde_yaml::to_string(&value)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    const BASIC: &str = "function main(c) { c['mixed-port'] = 7899; return c; }";

    fn worker_command() -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args([
            "--exact",
            "script_override::tests::worker_process",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ]);
        command
    }

    // Test harness entry only; the parent still runs the production transport,
    // protocol, allocator guard and evaluator in a real isolated process.
    #[test]
    #[ignore = "subprocess entry, invoked by script tests"]
    fn worker_process() {
        match std::env::var("CROSSBOW_TEST_WORKER_MODE").as_deref() {
            Ok("empty") => std::process::exit(0),
            Ok("crash") => std::process::exit(17),
            Ok("hang") => {
                std::thread::sleep(Duration::from_secs(30));
                std::process::exit(0);
            }
            _ => script_worker_entry(),
        }
    }

    #[test]
    fn basic_transform_roundtrip() {
        let out = run_script(BASIC, "mixed-port: 7890\nproxies: []\n").unwrap();
        assert!(out.contains("mixed-port: 7899"));
        assert!(out.contains("proxies: []"));
    }

    #[test]
    fn drops_invalid_keys_and_keeps_rules() {
        let out = run_script(
            "function main(c) { c.rules = ['MATCH,DIRECT']; c['new-key'] = 42; return c; }",
            "proxies: []",
        )
        .unwrap();
        assert!(out.contains("MATCH,DIRECT"));
        assert!(out.contains("new-key: 42"));
    }

    #[test]
    fn syntax_error_is_reported() {
        assert!(matches!(
            run_script("function main(c) { c.x = ; }", "a: 1"),
            Err(ScriptError::Runtime(_))
        ));
    }

    #[test]
    fn non_object_return_rejected() {
        assert!(matches!(
            run_script("function main(c) { return 42; }", "a: 1"),
            Err(ScriptError::NotAnObject)
        ));
    }

    #[test]
    fn chain_dispatch_script() {
        assert!(run_override(OverrideKind::Script, BASIC, "mixed-port: 1")
            .unwrap()
            .contains("mixed-port: 7899"));
        assert!(crate::override_patch::run_override_kind(
            OverrideKind::Script,
            BASIC,
            "mixed-port: 1"
        )
        .unwrap()
        .contains("mixed-port: 7899"));
    }

    #[test]
    fn infinite_loop_hits_budget_without_hanging() {
        assert!(matches!(
            run_script("function main(c) { while (true) {} }", "a: 1"),
            Err(ScriptError::Budget)
        ));
    }

    #[test]
    fn deep_recursion_hits_budget() {
        assert!(matches!(
            run_script(
                "function main(c) { function f() { return f(); } return f(); }",
                "a: 1"
            ),
            Err(ScriptError::Budget)
        ));
    }

    #[test]
    fn oversized_output_hits_budget() {
        assert!(matches!(
            run_script(
                "function main(c) { return {data:'x'.repeat(9000000)}; }",
                "a: 1"
            ),
            Err(ScriptError::Budget)
        ));
    }

    #[test]
    fn native_allocation_is_rejected_before_requesting_a_gigabyte() {
        assert!(matches!(
            run_script(
                "function main(c) { c.x = new ArrayBuffer(1073741824); return c; }",
                "a: 1"
            ),
            Err(ScriptError::Budget)
        ));
        assert!(run_script(BASIC, "a: 1").is_ok());
    }

    #[test]
    fn worker_errors_never_fall_back_to_host_evaluation() {
        for mode in ["empty", "crash"] {
            let mut command = worker_command();
            command.env("CROSSBOW_TEST_WORKER_MODE", mode);
            assert!(matches!(
                run_with_command(
                    command,
                    "function main(c) { throw new Error('SHOULD_NOT_EXECUTE'); }",
                    "{}",
                    WORKER_TIMEOUT
                ),
                Err(ScriptError::Worker)
            ));
        }
        let missing = tempfile::tempdir()
            .unwrap()
            .path()
            .join("missing-secret-worker");
        let error =
            run_with_command(Command::new(missing), BASIC, "{}", WORKER_TIMEOUT).unwrap_err();
        assert!(matches!(error, ScriptError::Worker));
        assert!(!error.to_string().contains("secret"));
    }

    #[test]
    fn hard_deadline_releases_real_child_process() {
        let mut command = worker_command();
        command.env("CROSSBOW_TEST_WORKER_MODE", "hang");
        let started = Instant::now();
        assert!(matches!(
            run_with_command(command, BASIC, "{}", Duration::from_millis(300)),
            Err(ScriptError::Timeout)
        ));
        assert!(started.elapsed() < Duration::from_secs(4));
        assert!(run_script(BASIC, "a: 1").is_ok());
    }

    #[test]
    fn realistic_large_config_retains_nodes_and_rules() {
        let mut config = String::from("proxies:\n");
        for i in 0..1000 {
            config.push_str(&format!(" - {{name: node-{i}, type: ss, server: node-{i}.example, port: 443, cipher: aes-128-gcm, password: demo}}\n"));
        }
        config.push_str("rules: [MATCH,DIRECT]\n");
        let out = run_script(BASIC, &config).unwrap();
        let value: serde_yaml::Value = serde_yaml::from_str(&out).unwrap();
        assert_eq!(value["proxies"].as_sequence().unwrap().len(), 1000);
        assert_eq!(value["rules"].as_sequence().unwrap().len(), 2);
        assert_eq!(value["mixed-port"].as_u64(), Some(7899));
    }

    #[test]
    fn input_budget_is_checked_before_spawning() {
        assert!(matches!(
            run_with_command(
                Command::new("missing"),
                &"x".repeat(SCRIPT_LIMIT + 1),
                "{}",
                WORKER_TIMEOUT
            ),
            Err(ScriptError::Budget)
        ));
        assert!(matches!(
            run_with_command(
                Command::new("missing"),
                BASIC,
                &"x".repeat(CONFIG_LIMIT + 1),
                WORKER_TIMEOUT
            ),
            Err(ScriptError::Budget)
        ));
    }

    #[test]
    fn error_messages_are_private_and_cannot_forge_budget_classification() {
        let error = run_script(
            "function main(c) { throw new Error('iteration loops SECRET_SUBSCRIPTION'); }",
            "{}",
        )
        .unwrap_err();
        assert!(matches!(error, ScriptError::Runtime(_)));
        assert!(!error.to_string().contains("SECRET"));
    }

    #[test]
    fn worker_protocol_rejects_missing_or_ambiguous_output() {
        assert!(matches!(parse_worker_output(""), Err(ScriptError::Worker)));
        assert!(matches!(
            parse_worker_output("unframed output"),
            Err(ScriptError::Worker)
        ));
        let valid = format!("{FRAME}{{\"ok\":true,\"result\":\"a: 1\"}}");
        assert_eq!(parse_worker_output(&valid).unwrap(), "a: 1");
        assert!(matches!(
            parse_worker_output(&format!("{valid}\n{valid}")),
            Err(ScriptError::Worker)
        ));
        assert!(matches!(
            parse_worker_output(&format!("{FRAME}{{\"ok\":false,\"kind\":\"budget\"}}")),
            Err(ScriptError::Budget)
        ));
    }
}
