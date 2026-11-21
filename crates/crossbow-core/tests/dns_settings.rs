use crossbow_core::{
    dns_settings::{validate, validate_for_engine, DnsMode, DnsSettings},
    render_config, Engine, Profile, ProfileKind, StoreSchema,
};

fn schema(engine: Engine) -> StoreSchema {
    let mut data = StoreSchema::default();
    data.engine.engine = engine;
    data.engine.flag_emoji = false;
    data.active_profile = Some("dns-test".into());
    data.profiles.push(Profile {
        id: "dns-test".into(), name: "DNS test".into(), kind: ProfileKind::Local, url: None,
        content: "proxies:\n  - {name: test-node, type: ss, server: 127.0.0.1, port: 19999, cipher: aes-128-gcm, password: test-only}\nproxy-groups:\n  - {name: Proxy, type: select, proxies: [test-node]}\nrules:\n  - DOMAIN-SUFFIX,direct.test,DIRECT\n  - MATCH,Proxy\n".into(),
        update_interval_min: 0, override_ids: vec![], last_updated: None, traffic: None, last_update: None,
    });
    data
}
fn custom() -> DnsSettings {
    DnsSettings {
        mode: DnsMode::Custom,
        proxy_nameservers: vec!["https://resolver.test:8443/dns-query".into()],
        direct_nameservers: vec!["udp://127.0.0.1:15353".into()],
        bootstrap_nameservers: vec![],
    }
}

#[test]
fn inherited_dns_preserves_configuration_and_old_store_defaults() {
    let mut data = schema(Engine::Mihomo);
    let original = render_config(&data).unwrap().config;
    data.dns.proxy_nameservers = vec!["invalid but inactive".into()];
    assert_eq!(render_config(&data).unwrap().config, original);
    let mut old = serde_json::to_value(&data).unwrap();
    old.as_object_mut().unwrap().remove("dns");
    assert_eq!(
        serde_json::from_value::<StoreSchema>(old).unwrap().dns,
        DnsSettings::default()
    );
}
#[test]
fn mihomo_enables_and_routes_upstreams_without_discarding_internal_policy() {
    let mut data = schema(Engine::Mihomo);
    data.profiles[0].content.push_str("dns:\n  enable: false\n  enhanced-mode: redir-host\n  nameserver-policy: {'+.lan': 10.0.0.53}\nhosts: {intranet.test: 10.0.0.2}\n");
    data.dns = custom();
    let rendered = render_config(&data).unwrap().config;
    let yaml: serde_yaml::Value = serde_yaml::from_str(&rendered).unwrap();
    assert_eq!(yaml["dns"]["enable"].as_bool(), Some(true));
    assert_eq!(
        yaml["dns"]["nameserver"][0].as_str(),
        Some("https://resolver.test:8443/dns-query#Proxy")
    );
    assert_eq!(
        yaml["dns"]["proxy-server-nameserver"][0].as_str(),
        Some("udp://127.0.0.1:15353#DIRECT")
    );
    assert_eq!(
        yaml["dns"]["direct-nameserver"][0].as_str(),
        Some("udp://127.0.0.1:15353#DIRECT")
    );
    assert_eq!(
        yaml["dns"]["nameserver-policy"]["+.lan"].as_str(),
        Some("10.0.0.53")
    );
    assert_eq!(yaml["hosts"]["intranet.test"].as_str(), Some("10.0.0.2"));
    let final_config = crossbow_core::apply_runtime(
        &rendered,
        &crossbow_core::RuntimeConfig {
            tun_enable: true,
            ..Default::default()
        },
    )
    .unwrap();
    let final_yaml: serde_yaml::Value = serde_yaml::from_str(&final_config).unwrap();
    assert_eq!(final_yaml["dns"]["nameserver"], yaml["dns"]["nameserver"]);
}
#[test]
fn singbox_preserves_port_path_and_resolves_upstream_without_cycles() {
    let mut data = schema(Engine::SingBox);
    data.dns = custom();
    let json: serde_json::Value =
        serde_json::from_str(&render_config(&data).unwrap().config).unwrap();
    let server = &json["dns"]["servers"][0];
    assert_eq!(server["server_port"], 8443);
    assert!(server.get("port").is_none());
    assert_eq!(server["domain_resolver"], "local-dns");
    assert_eq!(server["detour"], "Proxy");
    assert_eq!(server["path"], "/dns-query");
    assert!(
        server.get("tls").is_none(),
        "TLS verification must keep the secure default"
    );
    assert_eq!(json["dns"]["servers"][1]["server_port"], 15353);
    assert!(json["dns"]["servers"][1].get("detour").is_none());
}
#[test]
fn proxy_dns_without_bootstrap_cannot_be_saved_before_a_profile_exists() {
    let dns = DnsSettings {
        mode: DnsMode::Custom,
        proxy_nameservers: vec!["https://resolver.test".into()],
        ..Default::default()
    };
    assert!(validate_for_engine(&dns, Engine::Mihomo).is_err());
}

#[test]
fn unsupported_settings_never_succeed_silently() {
    let mut dns = custom();
    dns.bootstrap_nameservers.push("1.1.1.1".into());
    assert!(validate_for_engine(&dns, Engine::SingBox)
        .unwrap_err()
        .contains("引导"));
    dns.bootstrap_nameservers.clear();
    dns.proxy_nameservers.push("8.8.8.8".into());
    assert!(validate_for_engine(&dns, Engine::SingBox)
        .unwrap_err()
        .contains("多上游"));
    dns.proxy_nameservers.pop();
    dns.direct_nameservers = vec!["https://recursive.test/dns-query".into()];
    assert!(validate_for_engine(&dns, Engine::SingBox).is_err());
    let mut data = schema(Engine::SingBox);
    data.profiles[0].content =
        r#"{"dns":{"servers":[]},"outbounds":[{"type":"direct","tag":"DIRECT"}]}"#.into();
    data.dns = custom();
    assert!(render_config(&data)
        .err()
        .unwrap()
        .to_string()
        .contains("原生 sing-box"));
}
#[test]
fn invalid_urls_are_rejected_without_echoing_credentials_or_dropping_parts() {
    for raw in [
        "https://user:private-token@host/dns-query",
        "https://host/path?token=private-token",
        "https://host#private-token",
        "udp://1.1.1.1:0",
        "tls://host/path",
        "udp://[::1]junk",
        "udp://::1",
        "udp://host:abc",
        "udp://[not-ip]",
        "https://has space/path",
    ] {
        let mut dns = custom();
        dns.proxy_nameservers = vec![raw.into()];
        let error = validate(&dns).unwrap_err();
        assert!(!error.contains("private-token"));
    }
}

#[test]
#[ignore = "requires explicit paths to installed mihomo and sing-box; checks only isolated synthetic configs"]
fn native_cores_accept_custom_dns_ipv4_ipv6_and_nondefault_ports() {
    let dir = tempfile::tempdir().unwrap();
    for (engine, variable, filename) in [
        (Engine::Mihomo, "CB_MIHOMO_BINARY", "mihomo.yaml"),
        (Engine::SingBox, "CB_SINGBOX_BINARY", "singbox.json"),
    ] {
        let binary = std::env::var(variable).expect(variable);
        for address in ["udp://127.0.0.1:15353", "udp://[::1]:15353"] {
            let mut data = schema(engine);
            data.dns = custom();
            data.dns.direct_nameservers = vec![address.into()];
            let path = dir.path().join(filename);
            std::fs::write(&path, render_config(&data).unwrap().config).unwrap();
            let mut command = std::process::Command::new(&binary);
            if engine == Engine::Mihomo {
                command
                    .args(["-t", "-f"])
                    .arg(&path)
                    .arg("-d")
                    .arg(dir.path());
            } else {
                command.arg("check").arg("-c").arg(&path);
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{engine:?} {address}: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
