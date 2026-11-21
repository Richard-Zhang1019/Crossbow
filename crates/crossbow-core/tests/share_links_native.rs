//! Optional fixed-core checks of the public import entry; no real server/network use.
use crossbow_core::subscription::normalize_profile_content;
#[test]
#[ignore = "requires CB_MIHOMO_BINARY; config checking does not prove server connectivity"]
fn imported_protocol_subsets_pass_native_mihomo_check() {
    let binary = std::env::var("CB_MIHOMO_BINARY").expect("CB_MIHOMO_BINARY");
    let dir = tempfile::tempdir().unwrap();
    let id = "00000000-0000-4000-8000-000000000001";
    for input in [
        "hysteria2://a%3Ab%40c@[::1]?obfs=salamander&obfs-password=a%2Bb&sni=example.test#Hy2".to_string(),
        format!("tuic://{id}:a%3Ab%40c@127.0.0.1:443?alpn=h3&congestion_control=bbr&udp_relay_mode=native#TUIC"),
        format!("vless://{id}@127.0.0.1:443?security=reality&pbk=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA%3D&sid=abcd&fp=chrome&flow=xtls-rprx-vision#REALITY"),
    ] {
        let config = normalize_profile_content(&input).unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, config).unwrap();
        let output = std::process::Command::new(&binary)
            .arg("-t").arg("-f").arg(&path).arg("-d").arg(dir.path()).output().unwrap();
        assert!(output.status.success(), "synthetic config rejected: {} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    }
}
