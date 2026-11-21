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

#[test]
#[ignore = "fixture exporter for verify-share-links-native.py; explicit temp paths only"]
fn export_loopback_clients() {
    let source = std::env::var("CB_SHARE_INPUT").expect("CB_SHARE_INPUT");
    let output = std::env::var("CB_SHARE_OUTPUT").expect("CB_SHARE_OUTPUT");
    let cases: serde_json::Value = serde_json::from_slice(&std::fs::read(source).unwrap()).unwrap();
    for case in cases.as_array().unwrap() {
        let yaml = normalize_profile_content(case["uri"].as_str().unwrap()).unwrap();
        let mut config: serde_json::Value = serde_yaml::from_str(&yaml).unwrap();
        config["mixed-port"] = case["mixed_port"].clone();
        config["bind-address"] = serde_json::json!("127.0.0.1");
        config["allow-lan"] = serde_json::json!(false);
        config["log-level"] = serde_json::json!("warning");
        if case["engine"] == "singbox" {
            config = serde_json::from_str(
                &crossbow_core::singbox_convert::convert_to_singbox(
                    &yaml,
                    case["mixed_port"].as_u64().unwrap() as u16,
                    false,
                    None,
                )
                .unwrap(),
            )
            .unwrap();
        }
        std::fs::write(
            std::path::Path::new(&output).join(format!("{}.json", case["name"].as_str().unwrap())),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn pinned_client_export_structures_preserve_fields_and_wrapped_lists() {
    use base64::Engine as _;
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/share-links/clients.json")).unwrap();
    let mut links = vec![];
    for case in cases.as_array().unwrap() {
        let uri = case["uri"].as_str().unwrap();
        links.push(uri);
        let config: serde_json::Value =
            serde_yaml::from_str(&normalize_profile_content(uri).unwrap()).unwrap();
        let node = &config["proxies"][0];
        assert_eq!(node["type"], case["type"]);
        assert_eq!(node["password"], case["password"]);
        assert_eq!(node["server"], case["server"]);
        assert_eq!(node["sni"], "server.example");
        for (field, expected) in case["preserved"].as_object().unwrap() {
            assert_eq!(&node[field], expected, "field {field}");
        }
    }
    let list = links.join("\n");
    let direct = normalize_profile_content(&list).unwrap();
    for engine in [
        base64::engine::general_purpose::STANDARD,
        base64::engine::general_purpose::URL_SAFE_NO_PAD,
    ] {
        assert_eq!(
            normalize_profile_content(&engine.encode(&list)).unwrap(),
            direct
        );
    }
    assert!(normalize_profile_content(&(list + "\ntuic://invalid")).is_err());
}
