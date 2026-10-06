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
    // 动态取空闲端口：app 本体运行时会占用 7897，固定端口必然撞车
    let free = |s: &str| {
        std::net::TcpListener::bind(("127.0.0.1", 0))
            .and_then(|l| l.local_addr())
            .map(|a| a.port())
            .unwrap_or_else(|_| panic!("no free port for {s}"))
    };
    let (mixed, ctrl) = (free("mixed"), free("controller"));
    let final_cfg = apply_runtime(
        &rendered.config,
        &RuntimeConfig {
            engine: Engine::SingBox,
            mixed_port: mixed,
            allow_lan: false,
            controller_port: ctrl,
            controller_secret: secret.into(),
            log_level: "info".into(),
        },
    )
    .expect("apply_runtime");
    let work = std::path::Path::new("/tmp/cb-smoke-work");
    // 保留目录以复用 cache.db 里缓存的 remote 规则集（真实二次启动场景）；
    // 首次运行要经 gh-proxy 下载，可能显著超过常规启动时间
    std::fs::create_dir_all(work).unwrap();
    std::fs::write(work.join("config.yaml"), &final_cfg).unwrap();

    let bin = dir.join("binaries").join("sing-box");
    let mut child = std::process::Command::new(&bin)
        .arg("run").arg("-D").arg(work).arg("-c").arg(work.join("config.yaml"))
        .stdout(std::fs::File::create("/tmp/cb-smoke-out.log").unwrap())
        .stderr(std::fs::File::create("/tmp/cb-smoke-err.log").unwrap())
        .spawn()
        .expect("spawn sing-box");
    // 最多 60s 探测 clash api /version（首启含规则集下载）
    let mut ok = false;
    for _ in 0..300 {
        if let Ok(out) = std::process::Command::new("curl")
            .args(["-s", "-m", "1", "-H", &format!("Authorization: Bearer {secret}"),
                   &format!("http://127.0.0.1:{ctrl}/version")])
            .output()
        {
            let body = String::from_utf8_lossy(&out.stdout).to_string();
            if body.contains("version") { eprintln!("clash api: {body}"); ok = true; break; }
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(ok, "sing-box clash api 60s 内未就绪（首启含规则集下载）");
}
