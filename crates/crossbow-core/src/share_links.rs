//! Local, all-or-nothing conversion of share-link subscriptions. Never log input.
use crate::{Error, Result};
use base64::{
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
    Engine,
};
use serde_json::{json, Value};

fn bad(reason: &str) -> Error {
    Error::Network(format!(
        "分享订阅：{reason}（支持 ss/vmess/trojan/vless/hy2/tuic；TCP/WS/gRPC 仅限对应协议；不支持的参数不会被忽略）"
    ))
}
fn decode(s: &str) -> Result<String> {
    let data: String = s.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    for engine in [&STANDARD, &STANDARD_NO_PAD, &URL_SAFE, &URL_SAFE_NO_PAD] {
        if let Ok(bytes) = engine.decode(&data) {
            return String::from_utf8(bytes).map_err(|_| bad("Base64 内容不是 UTF-8"));
        }
    }
    Err(bad("无效 Base64 编码"))
}
fn unescape(s: &str) -> Result<String> {
    percent_encoding::percent_decode_str(s)
        .decode_utf8()
        .map(|s| s.into_owned())
        .map_err(|_| bad("无效 UTF-8 转义"))
}
fn uuid(s: &str) -> bool {
    if s.len() == 36 && ![8, 13, 18, 23].iter().all(|&i| s.as_bytes()[i] == b'-') {
        return false;
    }
    let hex = s.replace('-', "");
    hex.len() == 32
        && hex.bytes().all(|b| b.is_ascii_hexdigit())
        && (s.len() == 32 || s.len() == 36)
}
fn required<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| bad(&format!("vmess 缺少字段 {key}")))
}
fn transport(node: &mut Value, network: &str, path: &str, host: &str) -> Result<()> {
    match network {
        "" | "tcp" => {
            if !path.is_empty() || !host.is_empty() {
                return Err(bad("TCP 携带未支持的 Host/path 伪装"));
            }
        }
        "ws" => {
            node["network"] = json!("ws");
            node["ws-opts"] = json!({"path":if path.is_empty(){"/"}else{path}});
            if !host.is_empty() {
                node["ws-opts"]["headers"] = json!({"Host":host});
            }
        }
        "grpc" => {
            node["network"] = json!("grpc");
            node["grpc-opts"] = json!({"grpc-service-name":path});
        }
        _ => return Err(bad("传输类型暂不支持")),
    }
    Ok(())
}
fn vmess(raw: &str) -> Result<Value> {
    let body = raw.split('#').next().unwrap_or(raw);
    let v: Value =
        serde_json::from_str(&decode(body)?).map_err(|_| bad("vmess Base64 不是有效 JSON"))?;
    let object = v.as_object().ok_or_else(|| bad("vmess JSON 必须是对象"))?;
    for key in object.keys() {
        if ![
            "v",
            "ps",
            "add",
            "port",
            "id",
            "aid",
            "scy",
            "net",
            "type",
            "host",
            "path",
            "tls",
            "sni",
            "allowInsecure",
            "udp",
        ]
        .contains(&key.as_str())
        {
            return Err(bad("vmess 含暂不支持的扩展字段"));
        }
    }
    for key in ["ps", "scy", "net", "type", "host", "path", "tls", "sni"] {
        if v.get(key).is_some_and(|value| !value.is_string()) {
            return Err(bad("vmess 文本字段类型错误"));
        }
    }
    let id = required(&v, "id")?;
    if !uuid(id) {
        return Err(bad("vmess UUID 无效"));
    }
    let port = v["port"]
        .as_u64()
        .or_else(|| v["port"].as_str()?.parse().ok())
        .filter(|p| *p > 0 && *p <= 65535)
        .ok_or_else(|| bad("vmess 端口无效"))?;
    let header = v["type"].as_str().unwrap_or("");
    if !matches!(header, "" | "none") {
        return Err(bad("vmess 传输伪装 type 暂不支持"));
    }
    let aid = match v.get("aid") {
        None => 0,
        Some(x) => x
            .as_u64()
            .or_else(|| x.as_str()?.parse().ok())
            .ok_or_else(|| bad("vmess alterId 无效"))?,
    };
    let tls = v["tls"].as_str().unwrap_or("");
    if !matches!(tls, "" | "none" | "tls") {
        return Err(bad("vmess TLS 类型暂不支持"));
    }
    let mut node = json!({"name":v["ps"].as_str().unwrap_or("VMess"),"type":"vmess","server":required(&v,"add")?,"port":port,"uuid":id,"alterId":aid,"cipher":v["scy"].as_str().unwrap_or("auto"),"tls":tls=="tls"});
    for (source, target) in [("allowInsecure", "skip-cert-verify"), ("udp", "udp")] {
        if let Some(value) = v.get(source) {
            let value = value
                .as_bool()
                .or_else(|| match value.as_str() {
                    Some("1" | "true") => Some(true),
                    Some("0" | "false") => Some(false),
                    _ => None,
                })
                .ok_or_else(|| bad("vmess 布尔字段类型错误"))?;
            node[target] = json!(value);
        }
    }
    if let Some(sni) = v["sni"].as_str().filter(|s| !s.is_empty()) {
        node["servername"] = json!(sni);
    }
    if v["net"].as_str() == Some("grpc") && v["host"].as_str().is_some_and(|host| !host.is_empty())
    {
        return Err(bad("vmess gRPC Host 暂不支持"));
    }
    transport(
        &mut node,
        v["net"].as_str().unwrap_or("tcp"),
        v["path"].as_str().unwrap_or(""),
        v["host"].as_str().unwrap_or(""),
    )?;
    Ok(node)
}
fn uri(line: &str) -> Result<Value> {
    let (scheme, raw) = line
        .split_once("://")
        .ok_or_else(|| bad("节点不是分享链接"))?;
    if scheme == "vmess" {
        return vmess(raw);
    }
    if !matches!(
        scheme,
        "ss" | "trojan" | "vless" | "hy2" | "hysteria2" | "tuic"
    ) {
        return Err(bad(
            "未支持的 URI 协议；当前支持 ss/vmess/trojan/vless/hy2(hysteria2)/tuic",
        ));
    }
    // Legacy SS encodes the entire method:password@host:port payload.
    let full;
    let input = if scheme == "ss" && !raw.split('#').next().unwrap_or(raw).contains('@') {
        let (encoded, fragment) = raw.split_once('#').unwrap_or((raw, ""));
        full = format!("ss://{}#{}", decode(encoded)?, fragment);
        full.as_str()
    } else {
        line
    };
    let u = url::Url::parse(input).map_err(|_| bad("URI 结构无效"))?;
    let server = u
        .host_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| bad("缺少服务器地址"))?
        .trim_start_matches('[')
        .trim_end_matches(']');
    let port = match u.port() {
        Some(port) if port > 0 => port,
        None if matches!(scheme, "hy2" | "hysteria2") => 443,
        _ => return Err(bad("缺少端口或端口无效")),
    };
    if !matches!(u.path(), "" | "/") {
        return Err(bad("URI 路径未通过 path 查询参数指定"));
    }
    let known: &[&str] = match scheme {
        "hy2" | "hysteria2" => &["sni", "insecure", "obfs", "obfs-password", "pinSHA256"],
        "tuic" => &[
            "sni",
            "alpn",
            "congestion_control",
            "udp_relay_mode",
            "insecure",
            "allow_insecure",
            "allowInsecure",
        ],
        "vless" => &[
            "type",
            "security",
            "sni",
            "peer",
            "host",
            "path",
            "serviceName",
            "encryption",
            "flow",
            "allowInsecure",
            "skip-cert-verify",
            "pbk",
            "sid",
            "spx",
            "fp",
        ],
        _ => &[
            "type",
            "security",
            "sni",
            "peer",
            "host",
            "path",
            "serviceName",
            "encryption",
            "flow",
            "allowInsecure",
            "skip-cert-verify",
        ],
    };
    let mut params = std::collections::BTreeMap::new();
    for (key, value) in u.query_pairs() {
        if !known.contains(&key.as_ref()) {
            // Parameter names are untrusted too. Only identify known capability
            // names; never reflect arbitrary query data in user-facing errors.
            let field = match key.as_ref() {
                "type" | "zero_rtt" | "unknownParam" => key.as_ref(),
                _ => "未知查询参数",
            };
            return Err(bad(&format!("该协议参数 {field} 暂不支持")));
        }
        if params.insert(key.to_string(), value.to_string()).is_some() {
            return Err(bad("重复的 URI 查询参数"));
        }
    }
    let get = |k: &str| params.get(k).map(String::as_str).unwrap_or("");
    let name = unescape(u.fragment().unwrap_or(scheme))?;
    let mut node = json!({"name":name,"type":scheme,"server":server,"port":port});
    match scheme {
        "ss" => {
            if !params.is_empty() {
                return Err(bad("ss 插件/传输参数暂不支持"));
            }
            let auth = if let Some(pass) = u.password() {
                format!("{}:{}", unescape(u.username())?, unescape(pass)?)
            } else {
                decode(&unescape(u.username())?)?
            };
            let (cipher, password) = auth
                .split_once(':')
                .filter(|(c, p)| !c.is_empty() && !p.is_empty())
                .ok_or_else(|| bad("ss 缺少加密方法或密码"))?;
            node["cipher"] = json!(cipher);
            node["password"] = json!(password);
        }
        "trojan" => {
            if params.contains_key("encryption") || params.contains_key("flow") {
                return Err(bad("trojan 不支持 encryption/flow 参数"));
            }
            if u.password().is_some() {
                return Err(bad("trojan 密码中的冒号需要 URL 转义"));
            }
            let pass = unescape(u.username())?;
            if pass.is_empty() {
                return Err(bad("trojan 缺少密码"));
            }
            node["password"] = json!(pass);
            if !matches!(get("security"), "" | "tls") {
                return Err(bad("trojan 仅支持 TLS"));
            }
        }
        "vless" => {
            let id = unescape(u.username())?;
            if !uuid(&id) || u.password().is_some() {
                return Err(bad("vless UUID 无效"));
            }
            if !matches!(get("encryption"), "" | "none") {
                return Err(bad("vless encryption 暂不支持"));
            }
            node["uuid"] = json!(id);
        }
        "hy2" | "hysteria2" | "tuic" => {}
        _ => return Err(bad("未支持的 URI 协议")),
    }
    // ---- Hy2 / TUIC：UDP 类协议，无传输层参数（ws/grpc 等不存在）----
    if matches!(scheme, "hy2" | "hysteria2" | "tuic") {
        let auth = unescape(u.username())?;
        match scheme {
            "tuic" => {
                // tuic://<uuid>:<password>@host:port
                let password = u.password().ok_or_else(|| bad("tuic 缺少 password"))?;
                let password = unescape(password)?;
                if !uuid(&auth) {
                    return Err(bad("tuic UUID 无效"));
                }
                if password.is_empty() {
                    return Err(bad("tuic 密码不能为空"));
                }
                node["uuid"] = json!(auth);
                node["password"] = json!(password);
            }
            _ => {
                // hy2://<auth>@host:port（单段密码；user:pass 双段为歧义，拒绝）
                if u.password().is_some() {
                    return Err(bad("hy2 认证应为单段 auth，密码中的冒号需 URL 转义"));
                }
                if auth.is_empty() {
                    return Err(bad("hy2 缺少认证密码"));
                }
                node["type"] = json!("hysteria2");
                node["password"] = json!(auth);
            }
        }
        let sni = get("sni");
        if !sni.is_empty() {
            node["sni"] = json!(sni);
            node["servername"] = json!(sni);
        }
        if scheme == "tuic" {
            match get("congestion_control") {
                "" => {}
                v @ ("bbr" | "cubic") => node["congestion-controller"] = json!(v),
                _ => return Err(bad("tuic congestion_control 暂不支持")),
            }
            match get("udp_relay_mode") {
                "" => {}
                v @ ("native" | "quic") => node["udp-relay-mode"] = json!(v),
                _ => return Err(bad("tuic udp_relay_mode 暂不支持")),
            }
            let alpn = get("alpn");
            if !alpn.is_empty() {
                let items: Vec<&str> = alpn
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect();
                if items.is_empty() {
                    return Err(bad("tuic alpn 为空"));
                }
                node["alpn"] = json!(items);
            }
            let mut insecure = None;
            for key in ["insecure", "allow_insecure", "allowInsecure"] {
                if let Some(value) = params.get(key) {
                    let value = match value.as_str() {
                        "1" | "true" => true,
                        "0" | "false" => false,
                        _ => return Err(bad("证书校验参数无效")),
                    };
                    if insecure.is_some_and(|previous| previous != value) {
                        return Err(bad("URI 证书校验别名参数互相冲突"));
                    }
                    insecure = Some(value);
                }
            }
            if let Some(value) = insecure {
                node["skip-cert-verify"] = json!(value);
            }
        } else {
            // hy2：混淆与证书校验
            let insecure = get("insecure");
            if !insecure.is_empty() {
                node["skip-cert-verify"] = json!(match insecure {
                    "1" | "true" => true,
                    "0" | "false" => false,
                    _ => return Err(bad("证书校验参数无效")),
                });
            }
            match get("obfs") {
                "" => {
                    if !get("obfs-password").is_empty() {
                        return Err(bad("hy2 obfs-password 需要 obfs=salamander"));
                    }
                }
                "salamander" => {
                    let obfs_pass = get("obfs-password");
                    if obfs_pass.is_empty() {
                        return Err(bad("hy2 obfs=salamander 缺少 obfs-password"));
                    }
                    node["obfs"] = json!("salamander");
                    node["obfs-password"] = json!(obfs_pass);
                }
                _ => return Err(bad("hy2 obfs 暂不支持")),
            }
            if params.contains_key("pinSHA256") {
                return Err(bad("hy2 pinSHA256（证书指纹固定）暂不支持"));
            }
        }
        return Ok(node);
    }
    if scheme != "ss" {
        for (a, b) in [("sni", "peer"), ("allowInsecure", "skip-cert-verify")] {
            if params.contains_key(a) && params.contains_key(b) && get(a) != get(b) {
                return Err(bad("URI 别名参数互相冲突"));
            }
        }
        if scheme == "vless" {
            if get("security") != "reality"
                && ["pbk", "sid", "spx"]
                    .iter()
                    .any(|key| params.contains_key(*key))
            {
                return Err(bad("pbk/sid/spx 仅支持 REALITY"));
            }
            if !matches!(get("security"), "tls" | "reality")
                && (!get("flow").is_empty() || params.contains_key("fp"))
            {
                return Err(bad("vless flow/fp 需要 TLS 或 REALITY"));
            }
            if !get("flow").is_empty() && !matches!(get("type"), "" | "tcp") {
                return Err(bad("vless vision 仅支持 TCP 传输"));
            }
        }
        // vless flow：仅 xtls-rprx-vision 有明确 mihomo 语义
        if scheme == "vless" {
            match get("flow") {
                "" => {}
                v @ "xtls-rprx-vision" => node["flow"] = json!(v),
                _ => return Err(bad("vless flow 暂不支持")),
            }
        }
        match (scheme, get("security")) {
            (_, "" | "none" | "tls") => {}
            ("vless", "reality") => {
                // REALITY：pbk 必须、sid 可选、spx（spiderX）无 mihomo 映射
                let pbk = get("pbk");
                if pbk.is_empty() {
                    return Err(bad("vless REALITY 缺少 pbk（public-key）"));
                }
                if !get("spx").is_empty() {
                    return Err(bad("vless REALITY spx 参数暂不支持"));
                }
                let key = URL_SAFE_NO_PAD
                    .decode(pbk)
                    .or_else(|_| URL_SAFE.decode(pbk))
                    .map_err(|_| bad("vless REALITY pbk 无效"))?;
                if key.len() != 32 {
                    return Err(bad("vless REALITY pbk 长度无效"));
                }
                let sid = get("sid");
                if sid.len() > 16
                    || sid.len() % 2 != 0
                    || !sid.bytes().all(|byte| byte.is_ascii_hexdigit())
                {
                    return Err(bad("vless REALITY sid 无效"));
                }
                node["tls"] = json!(true);
                let mut reality = json!({ "public-key": pbk });
                if !get("sid").is_empty() {
                    reality["short-id"] = json!(get("sid"));
                }
                node["reality-opts"] = reality;
            }
            _ => {
                return Err(bad("security 暂不支持（REALITY 仅 vless），不会降级为明文"));
            }
        }
        if scheme == "vless" {
            node["tls"] = json!(matches!(get("security"), "tls" | "reality"));
            // uTLS 指纹：mihomo client-fingerprint（tls/reality 均可携带）
            if !get("fp").is_empty() {
                node["client-fingerprint"] = json!(get("fp"));
            }
        }
        let sni = if get("sni").is_empty() {
            get("peer")
        } else {
            get("sni")
        };
        if !sni.is_empty() {
            node["sni"] = json!(sni);
            node["servername"] = json!(sni);
        }
        let insecure = if get("allowInsecure").is_empty() {
            get("skip-cert-verify")
        } else {
            get("allowInsecure")
        };
        if !insecure.is_empty() {
            node["skip-cert-verify"] = json!(match insecure {
                "1" | "true" => true,
                "0" | "false" => false,
                _ => return Err(bad("证书校验参数无效")),
            });
        }
        if get("type") != "grpc" && !get("serviceName").is_empty() {
            return Err(bad("serviceName 仅支持 gRPC"));
        }
        if get("type") == "grpc" && (!get("host").is_empty() || !get("path").is_empty()) {
            return Err(bad("gRPC Host/path 暂不支持，请使用 serviceName"));
        }
        let path = if get("type") == "grpc" {
            get("serviceName")
        } else {
            get("path")
        };
        transport(&mut node, get("type"), path, get("host"))?;
    }
    Ok(node)
}
/// Preserve complete configurations verbatim. Lists are normalized exactly once.
pub fn normalize(content: &str) -> Result<String> {
    if content.len() > 8 * 1024 * 1024 {
        return Err(bad("内容超过 8 MiB"));
    }
    let text = content.trim().trim_start_matches('\u{feff}').trim();
    if let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(text) {
        if doc.get("proxies").is_some()
            || doc.get("proxy-providers").is_some()
            || doc.get("outbounds").is_some()
        {
            if doc
                .get("outbounds")
                .and_then(|v| v.as_sequence())
                .is_some_and(|o| o.iter().any(|v| v.get("protocol").is_some()))
            {
                return Err(bad(
                    "检测到 V2Ray 原生 JSON，暂不支持其配置语义；请使用分享链接列表",
                ));
            }
            return Ok(content.to_owned());
        }
    }
    let (list, format) = if text.contains("://") {
        (text.to_owned(), "URI")
    } else {
        (
            decode(text).map_err(|_| {
                bad("无法识别格式；支持 Clash YAML、sing-box JSON、Base64 和分享链接列表")
            })?,
            "Base64",
        )
    };
    let mut nodes = Vec::new();
    let mut names = std::collections::HashSet::new();
    for (i, line) in list.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if nodes.len() >= 4096 {
            return Err(bad("节点数量超过 4096"));
        }
        let mut node = uri(line)
            .map_err(|error| bad(&format!("{format} 第 {} 行解析失败：{error}", i + 1)))?;
        let base = node["name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("Node")
            .to_owned();
        let mut name = base.clone();
        let mut suffix = 2;
        while matches!(name.as_str(), "DIRECT" | "REJECT" | "节点选择") || names.contains(&name)
        {
            name = format!("{base} ({suffix})");
            suffix += 1;
        }
        names.insert(name.clone());
        node["name"] = json!(name);
        nodes.push(node);
    }
    if nodes.is_empty() {
        return Err(bad("列表没有节点"));
    }
    let names: Vec<_> = nodes.iter().map(|n| n["name"].clone()).collect();
    serde_yaml::to_string(&json!({"proxies":nodes,"proxy-groups":[{"name":"节点选择","type":"select","proxies":names}],"rules":["MATCH,节点选择"]})).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "00000000-0000-4000-8000-000000000001";
    fn links() -> Vec<String> {
        vec![format!("ss://{}@ss.example:8388#SS",STANDARD_NO_PAD.encode("aes-128-gcm:p@ss:word")),
            format!("vmess://{}",STANDARD.encode(json!({"v":"2","ps":"VMess","add":"vm.example","port":"443","id":ID,"aid":"0","scy":"auto","net":"ws","type":"none","host":"front.example","path":"/ws","tls":"tls","sni":"front.example"}).to_string())),
            "trojan://password@tr.example:443?sni=front.example#Trojan".into(),
            format!("vless://{ID}@vl.example:443?encryption=none&security=tls&type=ws&host=front.example&path=%2Fws#VLESS")]
    }
    #[test]
    fn mixed_lists_and_wrapped_base64_convert_through_both_renderers() {
        let input = links().join("\r\n");
        let yaml = crate::subscription::normalize_profile_content(&input).unwrap();
        let names = crate::subscription::parse_proxy_names(&yaml).unwrap();
        assert_eq!(names, vec!["SS", "VMess", "Trojan", "VLESS"]);
        let doc: Value = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(doc["proxies"][0]["password"], "p@ss:word");
        assert_eq!(
            doc["proxies"][1]["ws-opts"]["headers"]["Host"],
            "front.example"
        );
        assert_eq!(doc["proxies"][3]["ws-opts"]["path"], "/ws");
        assert_eq!(doc["rules"][0], "MATCH,节点选择");
        let sing = crate::convert_to_singbox(&yaml, 7898, false, None).unwrap();
        let sing: Value = serde_json::from_str(&sing).unwrap();
        assert!(sing["outbounds"].as_array().unwrap().len() >= 4);
        for encoded in [STANDARD.encode(&input), URL_SAFE_NO_PAD.encode(&input)] {
            let wrapped = encoded
                .as_bytes()
                .chunks(73)
                .map(|b| std::str::from_utf8(b).unwrap())
                .collect::<Vec<_>>()
                .join("\n");
            assert_eq!(
                crate::subscription::normalize_profile_content(&wrapped).unwrap(),
                yaml
            );
        }
        assert_eq!(
            crate::subscription::normalize_profile_content(&yaml).unwrap(),
            yaml
        );
    }
    #[test]
    fn explicit_plaintext_on_port_443_stays_plaintext_in_singbox() {
        for input in [
            format!("vless://{ID}@host:443?security=none#Plain"),
            format!(
                "vmess://{}",
                STANDARD.encode(
                    json!({"add":"host","port":443,"id":ID,"ps":"Plain","tls":"none"}).to_string()
                )
            ),
        ] {
            let yaml = normalize(&input).unwrap();
            let sing: Value =
                serde_json::from_str(&crate::convert_to_singbox(&yaml, 7898, false, None).unwrap())
                    .unwrap();
            let node = sing["outbounds"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["tag"] == "Plain")
                .unwrap();
            assert!(
                node.get("tls").is_none(),
                "explicit TLS=false was overwritten"
            );
        }
    }
    #[test]
    fn ss_legacy_ipv6_percent_escaped_credentials_and_unique_names() {
        let legacy = format!(
            "ss://{}#%E6%B5%8B%E8%AF%95",
            URL_SAFE_NO_PAD.encode("aes-256-gcm:pass@[::1]:8388")
        );
        let plain = "ss://aes-128-gcm:p%40ss%3Aword@host.example:443#DIRECT";
        let doc: Value =
            serde_yaml::from_str(&normalize(&format!("{legacy}\n{plain}\n{plain}")).unwrap())
                .unwrap();
        assert_eq!(doc["proxies"][0]["server"], "::1");
        assert_eq!(doc["proxies"][0]["name"], "测试");
        assert_eq!(doc["proxies"][1]["password"], "p@ss:word");
        assert_ne!(doc["proxies"][1]["name"], doc["proxies"][2]["name"]);
        assert_ne!(doc["proxies"][1]["name"], "DIRECT");
    }
    // ---- #599：Hy2 / TUIC / VLESS REALITY / flow ----

    #[test]
    fn hy2_basic_and_obfs_roundtrip() {
        let out = normalize("hy2://p%40ss@hy.example:443?sni=hy.example&insecure=1#HY2").unwrap();
        let doc: Value = serde_yaml::from_str(&out).unwrap();
        let node = &doc["proxies"][0];
        assert_eq!(node["type"], "hysteria2");
        assert_eq!(node["server"], "hy.example");
        assert_eq!(node["port"], 443);
        assert_eq!(node["password"], "p@ss", "URL 转义还原");
        assert_eq!(node["sni"], "hy.example");
        assert_eq!(node["skip-cert-verify"], true);
        assert!(node.get("obfs").is_none());
    }

    #[test]
    fn hy2_obfs_salamander_pairs_and_rejects_orphans() {
        let out = normalize(
            "hysteria2://auth@hy.example:443?obfs=salamander&obfs-password=ob&insecure=0#n",
        )
        .unwrap();
        let doc: Value = serde_yaml::from_str(&out).unwrap();
        assert_eq!(doc["proxies"][0]["obfs"], "salamander");
        assert_eq!(doc["proxies"][0]["obfs-password"], "ob");
        assert_eq!(doc["proxies"][0]["skip-cert-verify"], false);

        // obfs-password 无 obfs → 冲突拒绝
        assert!(normalize("hy2://a@hy.example:443?obfs-password=x#n").is_err());
        // obfs 无 obfs-password → 拒绝
        assert!(normalize("hy2://a@hy.example:443?obfs=salamander#n").is_err());
        // pinSHA256 无 mihomo 映射 → 拒绝
        assert!(normalize("hy2://a@hy.example:443?pinSHA256=ABCD#n")
            .unwrap_err()
            .to_string()
            .contains("pinSHA256"));
        // 未知参数拒绝（不静默丢）
        assert!(normalize("hy2://a@hy.example:443?unknownParam=1#n").is_err());
        // 双段认证歧义拒绝
        assert!(normalize("hy2://user:pass@hy.example:443#n").is_err());
    }

    #[test]
    fn tuic_fields_map_and_reject_unknown() {
        let out = normalize(
        "tuic://00000000-0000-4000-8000-000000000001:secret@tu.example:443?sni=tu.example&congestion_control=bbr&udp_relay_mode=native&alpn=h3,hq&insecure=1#TUIC",
    )
    .unwrap();
        let doc: Value = serde_yaml::from_str(&out).unwrap();
        let node = &doc["proxies"][0];
        assert_eq!(node["type"], "tuic");
        assert_eq!(node["uuid"], "00000000-0000-4000-8000-000000000001");
        assert_eq!(node["password"], "secret");
        assert_eq!(node["sni"], "tu.example");
        assert_eq!(node["congestion-controller"], "bbr");
        assert_eq!(node["udp-relay-mode"], "native");
        assert_eq!(node["alpn"], serde_json::json!(["h3", "hq"]));
        assert_eq!(node["skip-cert-verify"], true);

        // 无效 UUID / 缺密码 / 非法拥塞控制 / 未知参数
        assert!(normalize("tuic://not-a-uuid:pw@tu.example:443#n").is_err());
        assert!(normalize("tuic://00000000-0000-4000-8000-000000000001@tu.example:443#n").is_err());
        assert!(normalize(
        "tuic://00000000-0000-4000-8000-000000000001:pw@tu.example:443?congestion_control=fast#n"
    )
    .unwrap_err()
    .to_string()
    .contains("congestion_control"));
        assert!(normalize(
            "tuic://00000000-0000-4000-8000-000000000001:pw@tu.example:443?zero_rtt=true#n"
        )
        .unwrap_err()
        .to_string()
        .contains("zero_rtt"));
    }

    #[test]
    fn vless_reality_and_vision_map_to_mihomo_fields() {
        let out = normalize(
        "vless://00000000-0000-4000-8000-000000000001@vl.example:443?encryption=none&security=reality&sni=vl.example&pbk=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA&sid=abcd&fp=chrome&flow=xtls-rprx-vision#REALITY",
    )
    .unwrap();
        let doc: Value = serde_yaml::from_str(&out).unwrap();
        let node = &doc["proxies"][0];
        assert_eq!(node["type"], "vless");
        assert_eq!(node["tls"], true);
        assert_eq!(node["servername"], "vl.example");
        assert_eq!(
            node["reality-opts"]["public-key"],
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        );
        assert_eq!(node["reality-opts"]["short-id"], "abcd");
        assert_eq!(node["client-fingerprint"], "chrome");
        assert_eq!(node["flow"], "xtls-rprx-vision");

        // 缺 pbk / spx / 非 vision flow 拒绝
        assert!(
        normalize(
            "vless://00000000-0000-4000-8000-000000000001@vl.example:443?security=reality&sni=vl.example#n"
        )
        .unwrap_err()
        .to_string()
        .contains("pbk")
    );
        assert!(
        normalize(
            "vless://00000000-0000-4000-8000-000000000001@vl.example:443?security=reality&pbk=K&spx=/spider#n"
        )
        .unwrap_err()
        .to_string()
        .contains("spx")
    );
        assert!(normalize(
            "vless://00000000-0000-4000-8000-000000000001@vl.example:443?flow=xtls-rprx-splice#n"
        )
        .unwrap_err()
        .to_string()
        .contains("flow"));
    }

    #[test]
    fn hy2_tuic_reject_transport_parameters() {
        // UDP 类协议没有 ws/grpc 传输层
        assert!(normalize("hy2://a@hy.example:443?type=ws&path=/x#n")
            .unwrap_err()
            .to_string()
            .contains("type"));
        assert!(normalize(
            "tuic://00000000-0000-4000-8000-000000000001:pw@tu.example:443?type=grpc#n"
        )
        .unwrap_err()
        .to_string()
        .contains("type"));
    }

    #[test]
    fn mixed_list_is_atomic_on_any_bad_line() {
        // 一行 hy2 有效 + 一行未知协议 → 整批失败（不自动跳过坏节点）
        let mixed = format!(
            "hy2://a@hy.example:443#OK\nvless://bad-uri-line\n{}",
            links()[0]
        );
        let err = normalize(&mixed).unwrap_err().to_string();
        assert!(err.contains("URI 第 2 行"), "{err}");
    }

    #[test]
    fn ipv6_server_and_url_escaped_name_roundtrip() {
        let out = normalize("hy2://auth@[2001:db8::1]:8443?sni=h.example#[H2] 名称").unwrap();
        let doc: Value = serde_yaml::from_str(&out).unwrap();
        assert_eq!(doc["proxies"][0]["server"], "2001:db8::1");
        assert_eq!(doc["proxies"][0]["port"], 8443);
        assert_eq!(doc["proxies"][0]["name"], "[H2] 名称");
    }

    #[test]
    fn hy2_default_port_and_tuic_security_aliases_are_validated() {
        let doc: Value =
            serde_yaml::from_str(&normalize("hy2://auth@h.example#n").unwrap()).unwrap();
        assert_eq!(doc["proxies"][0]["port"], 443);
        assert!(normalize("hy2://auth@h.example:0#n").is_err());
        let base = "tuic://00000000-0000-4000-8000-000000000001:pw@h.example:443";
        assert!(normalize(&format!("{base}?insecure=1&allowInsecure=0")).is_err());
        let out = normalize(&format!("{base}?allow_insecure=0&allowInsecure=false")).unwrap();
        let doc: Value = serde_yaml::from_str(&out).unwrap();
        assert_eq!(doc["proxies"][0]["skip-cert-verify"], false);
    }

    #[test]
    fn reality_and_vision_reject_incompatible_or_invalid_options() {
        for query in [
            "flow=xtls-rprx-vision",
            "security=none&flow=xtls-rprx-vision",
            "security=tls&flow=xtls-rprx-vision&type=ws",
            "security=none&fp=chrome",
            "security=tls&pbk=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "security=reality&pbk=bad",
            "security=reality&pbk=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA&sid=xyz",
        ] {
            assert!(normalize(&format!("vless://{ID}@h.example:443?{query}")).is_err());
        }
    }

    #[test]
    fn unsupported_values_do_not_escape_into_errors() {
        for link in [
            "hy2://auth@h.example:443?obfs=PRIVATE_VALUE".to_string(),
            format!("tuic://{ID}:pw@h.example:443?congestion_control=PRIVATE_VALUE"),
            format!("vless://{ID}@h.example:443?security=tls&flow=PRIVATE_VALUE"),
            "hy2://auth@h.example:443?PRIVATE_VALUE=1".to_string(),
        ] {
            let error = normalize(&link).unwrap_err().to_string();
            assert!(!error.contains("PRIVATE_VALUE"), "{error}");
        }
    }

    #[test]
    fn failures_are_all_or_nothing_format_specific_and_credential_free() {
        let malformed = "trojan://VERY_SECRET@host.example:bad?token=SUBSCRIPTION_SECRET";
        let error = normalize(&format!("{}\n{malformed}", links()[0]))
            .unwrap_err()
            .to_string();
        assert!(error.contains("URI 第 2 行"));
        assert!(!error.contains("VERY_SECRET"));
        assert!(!error.contains("SUBSCRIPTION_SECRET"));
        // REALITY 现已支持（#599）——但缺 pbk 仍拒绝（无歧义映射不静默丢）
        let reality_err = normalize(&format!("vless://{ID}@host:443?security=reality"))
            .unwrap_err()
            .to_string();
        assert!(reality_err.contains("pbk"), "{reality_err}");
        for link in [
            format!("vless://{ID}@host:443?type=quic"),
            "ss://YWVzLTEyOC1nY206cGFzcw@host:8388?plugin=obfs".into(),
        ] {
            assert!(normalize(&link)
                .unwrap_err()
                .to_string()
                .contains("暂不支持"));
        }
        assert!(normalize("PCFET0NUWVBFIGh0bWw+").is_err());
        assert!(normalize("{\"outbounds\":[{\"protocol\":\"vmess\"}]}")
            .unwrap_err()
            .to_string()
            .contains("V2Ray"));
    }
    #[test]
    fn unsupported_or_conflicting_options_are_never_silently_dropped() {
        for input in [
            "trojan://secret@host:443?flow=xtls-rprx-vision".to_owned(),
            "trojan://secret@host:443?sni=a&peer=b".to_owned(),
            "trojan://secret@host:443?allowInsecure=1&skip-cert-verify=0".to_owned(),
            format!("vless://{ID}@host:443?type=grpc&host=authority"),
            format!("vless://{ID}@host:443?type=ws&serviceName=grpc"),
            format!("vless://{ID}@host:443?type=tcp&type=ws"),
            format!(
                "vmess://{}",
                STANDARD.encode(
                    json!({"add":"host","port":443,"id":ID,"net":"grpc","host":"authority"})
                        .to_string()
                )
            ),
            format!(
                "vmess://{}",
                STANDARD.encode(json!({"add":"host","port":443,"id":ID,"fp":"chrome"}).to_string())
            ),
        ] {
            assert!(normalize(&input).is_err());
        }
        let doc: Value = serde_yaml::from_str(
            &normalize(&format!(
                "vless://{ID}@host:443?security=tls&type=grpc&serviceName=rpc"
            ))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(doc["proxies"][0]["grpc-opts"]["grpc-service-name"], "rpc");
    }
    #[test]
    fn complete_profiles_are_preserved_and_profile_lifecycle_uses_normalized_content() {
        let original = "proxies: []\n# keep comment\nrules: [MATCH,DIRECT]\n";
        assert_eq!(
            crate::subscription::normalize_profile_content(original).unwrap(),
            original
        );
        let native =
            r#"{"outbounds":[{"tag":"DIRECT","type":"direct"}],"route":{"final":"DIRECT"}}"#;
        assert_eq!(
            crate::subscription::normalize_profile_content(native).unwrap(),
            native
        );
        let mut schema = crate::StoreSchema::default();
        let profile = crate::Profile {
            id: "uri".into(),
            name: "Imported".into(),
            kind: crate::ProfileKind::Remote,
            url: Some("https://subscription.invalid".into()),
            content: normalize(&links()[0]).unwrap(),
            update_interval_min: 0,
            override_ids: vec![],
            last_updated: None,
            traffic: None,
            last_update: None,
        };
        schema.add_profile(profile).unwrap();
        schema.set_active_profile("uri").unwrap();
        schema
            .update_profile_content("uri", normalize(&links().join("\n")).unwrap(), 123, None)
            .unwrap();
        let rendered = crate::render_config(&schema).unwrap();
        assert_eq!(
            crate::subscription::parse_proxy_names(&rendered.config)
                .unwrap()
                .len(),
            4
        );
        let disk = serde_json::to_string(&schema).unwrap();
        let mut reopened: crate::StoreSchema = serde_json::from_str(&disk).unwrap();
        assert_eq!(reopened.active_profile.as_deref(), Some("uri"));
        reopened.remove_profile("uri").unwrap();
        assert!(reopened.profiles.is_empty());
    }
}

#[cfg(test)]
mod network_tests {
    use super::*;
    #[test]
    fn http_subscription_fetch_normalizes_and_preserves_traffic_header() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let body = STANDARD.encode("trojan://test-password@server.invalid:443#Test");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let count = stream.read(&mut request).unwrap();
            assert!(count > 0);
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Length: {}\r\nSubscription-Userinfo: upload=1; download=2; total=3\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        });
        let fetched =
            crate::subscription::fetch_subscription(&format!("http://{address}/subscription"), 2)
                .unwrap();
        server.join().unwrap();
        assert_eq!(
            crate::subscription::parse_proxy_names(&fetched.content).unwrap(),
            vec!["Test"]
        );
        assert_eq!(fetched.traffic.unwrap().download, 2);
    }
    #[test]
    fn http_failure_does_not_echo_subscription_credentials() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            assert!(stream.read(&mut request).unwrap() > 0);
            stream
                .write_all(
                    b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
        });
        let error = crate::subscription::fetch_subscription(
            &format!("http://{address}/PRIVATE_PATH?token=PRIVATE_TOKEN"),
            2,
        )
        .err()
        .expect("403 must fail")
        .to_string();
        server.join().unwrap();
        assert!(error.contains("HTTP 403"));
        assert!(!error.contains("PRIVATE_"));
        assert!(!error.contains(&address.to_string()));
    }
    #[test]
    #[ignore = "requires CROSSBOW_CORE_BIN; checks generated config with real mihomo"]
    fn real_mihomo_validates_converted_share_configuration() {
        let binary = std::env::var("CROSSBOW_CORE_BIN").expect("set CROSSBOW_CORE_BIN");
        let raw="ss://YWVzLTEyOC1nY206cGFzc3dvcmQ@127.0.0.1:8388#SS\ntrojan://password@127.0.0.1:443#Trojan\nvless://00000000-0000-4000-8000-000000000001@127.0.0.1:443?security=tls#VLESS";
        let vmess = STANDARD.encode(json!({"add":"127.0.0.1","port":443,"id":"00000000-0000-4000-8000-000000000001","ps":"VMess","net":"ws","path":"/ws","host":"front.invalid","tls":"tls"}).to_string());
        let config = normalize(&format!("{raw}\nvmess://{vmess}\nvless://00000000-0000-4000-8000-000000000001@127.0.0.1:443?security=tls&type=grpc&serviceName=rpc#GRPC")).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yaml");
        std::fs::write(&path, config).unwrap();
        let output = std::process::Command::new(binary)
            .args(["-t", "-f"])
            .arg(path)
            .arg("-d")
            .arg(directory.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "mihomo rejected generated configuration: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}
