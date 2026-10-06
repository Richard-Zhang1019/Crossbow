//! 用真实 store 数据验证 sing-box 转换链（CROSSBOW_STORE 指向 store.json）。

use crossbow_core::{render_config_with, Store};

#[test]
fn real_store_singbox_render() {
    let Ok(path) = std::env::var("CROSSBOW_STORE") else {
        eprintln!("skip: CROSSBOW_STORE not set");
        return;
    };
    let dir = std::path::Path::new(&path).parent().unwrap().to_path_buf();
    let mut store = Store::open(&dir).expect("open store");
    eprintln!("engine was: {:?}", store.data().engine.engine);
    store.data_mut().engine.engine = crossbow_core::Engine::SingBox;
    let rendered = match render_config_with(store.data(), true) {
        Ok(r) => r,
        Err(e) => panic!("render failed: {e}"),
    };
    eprintln!("config bytes: {}, head: {}", rendered.config.len(), &rendered.config[..rendered.config.len().min(300)]);
    assert!(rendered.config.trim_start().starts_with('{'), "should be JSON");
    std::fs::write("/tmp/crossbow-singbox-check.json", &rendered.config).unwrap();
}
