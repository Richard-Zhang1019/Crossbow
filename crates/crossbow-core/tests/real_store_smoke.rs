//! 端到端冒烟（CROSSBOW_STORE 门控）：渲染 → 注入 → 交给真实 sing-box 启动探测。

use crossbow_core::{apply_runtime, render_config_with, Engine, RuntimeConfig, Store};

#[test]
fn real_store_singbox_e2e() {
    let Ok(path) = std::env::var("CROSSBOW_STORE") else {
        eprintln!("skip: CROSSBOW_STORE not set");
        return;
    };
    let dir = std::path::Path::new(&path).parent().unwrap().to_path_buf();
    let mut store = Store::open(&dir).expect("open store");
    store.data_mut().engine.engine = Engine::SingBox;
    let rendered = render_config_with(store.data(), true).expect("render");
    let secret = "smoke-secret-123";
    let final_cfg = apply_runtime(
        &rendered.config,
        &RuntimeConfig {
            engine: Engine::SingBox,
            mixed_port: 7897,
            allow_lan: false,
            controller_port: 19091,
            controller_secret: secret.into(),
            log_level: "info".into(),
        },
    )
    .expect("apply_runtime");
    let work = std::path::Path::new("/tmp/cb-smoke-work");
    let _ = std::fs::remove_dir_all(work);
    std::fs::create_dir_all(work).unwrap();
    std::fs::write(work.join("config.yaml"), &final_cfg).unwrap();

    let bin = dir.join("binaries").join("sing-box");
    let mut child = std::process::Command::new(&bin)
        .arg("run").arg("-D").arg(work).arg("-c").arg(work.join("config.yaml"))
        .stdout(std::fs::File::create("/tmp/cb-smoke-out.log").unwrap())
        .stderr(std::fs::File::create("/tmp/cb-smoke-err.log").unwrap())
        .spawn()
        .expect("spawn sing-box");
    // 最多 5s 探测 clash api /version
    let mut ok = false;
    for _ in 0..25 {
        if let Ok(out) = std::process::Command::new("curl")
            .args(["-s", "-m", "1", "-H", &format!("Authorization: Bearer {secret}"),
                   "http://127.0.0.1:19091/version"])
            .output()
        {
            let body = String::from_utf8_lossy(&out.stdout).to_string();
            if body.contains("version") { eprintln!("clash api: {body}"); ok = true; break; }
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(ok, "sing-box clash api 5s 内未就绪");
}
