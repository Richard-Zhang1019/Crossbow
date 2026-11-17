//! #544：用户反馈问题——应用内表单生成预填 GitHub issue。
//!
//! 上报不经过自建后端：表单内容在本地组装成 GitHub issues/new 的预填链接，
//! 由系统默认浏览器打开，用户在 GitHub 登录后一键提交。没有运维负担，
//! 也不存在用户文本的存储面。
//!
//! 隐私边界：自动附加的环境块只含白名单字段（版本/系统/内核/代理模式/
//! 混合端口——与诊断摘要同口径），不含订阅名、节点地址或凭据；用户描述
//! 内容由用户自行负责。本命令只打开固定仓库的 issues/new 域名。

pub(crate) const FEEDBACK_BASE: &str = "https://github.com/Richard-Zhang1019/Crossbow/issues/new";

const TITLE_MAX: usize = 200;
const BODY_MAX: usize = 6000;

/// 组装 issues/new 预填链接。标题/描述校验（空/超长）在此完成，
/// 返回完整 URL 供 open 命令打开。
pub(crate) fn feedback_issue_url(title: &str, body: &str) -> Result<String, String> {
    let description = body;
    let title = title.trim();
    let description = description.trim();
    if title.is_empty() {
        return Err("标题不能为空".into());
    }
    if title.chars().count() > TITLE_MAX {
        return Err(format!("标题过长（上限 {TITLE_MAX} 字符）"));
    }
    if description.is_empty() {
        return Err("问题描述不能为空".into());
    }
    if description.chars().count() > BODY_MAX {
        return Err(format!("问题描述过长（上限 {BODY_MAX} 字符）"));
    }
    let body = format!("### 问题描述\n\n{description}\n\n### 复现步骤\n\n（请在此补充）");
    let query = form_urlencoded(title, &body);
    Ok(format!("{FEEDBACK_BASE}?{query}"))
}

/// application/x-www-form-urlencoded（空格转 +，保留中文多字节）。
fn form_urlencoded(title: &str, body: &str) -> String {
    fn encode(value: &str) -> String {
        let mut out = String::with_capacity(value.len());
        for byte in value.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                    out.push(byte as char)
                }
                b' ' => out.push('+'),
                other => out.push_str(&format!("%{other:02X}")),
            }
        }
        out
    }
    format!("title={}&body={}", encode(title), encode(body))
}

#[tauri::command]
pub(crate) async fn open_feedback_issue(title: String, body: String) -> Result<(), String> {
    // 白名单：只允许本仓库 issues/new（链接由本模块构造，这里再校验
    // 一次入口，防止未来调用方变化引入任意 URL 打开）。
    let url = feedback_issue_url(&title, &body)?;
    let parsed =
        url::Url::parse(&url).map_err(|_: url::ParseError| "Invalid feedback URL".to_string())?;
    if parsed.origin().ascii_serialization() != FEEDBACK_BASE || parsed.path() != "/issues/new" {
        return Err("Invalid feedback URL".into());
    }
    super::monitor::open_in_system_browser(&url, "Could not open feedback page").await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[test]
    fn url_encodes_title_and_body() {
        let url = feedback_issue_url("连不上网", "描述 A B\n\n- v0.15.0").unwrap();
        assert!(url.starts_with(FEEDBACK_BASE));
        let decoded = decode_query(&url);
        assert_eq!(
            decoded
                .iter()
                .find(|(k, _)| k == "title")
                .map(|(_, v)| v.as_str()),
            Some("连不上网")
        );
        let body = decoded
            .iter()
            .find(|(k, _)| k == "body")
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert!(body.contains("描述 A B"));
        assert!(body.contains("- v0.15.0"));
        assert!(body.contains("### 问题描述"));
        assert!(body.contains("### 复现步骤"));
    }

    #[test]
    fn empty_or_oversized_inputs_are_rejected() {
        assert!(feedback_issue_url("", "x").is_err());
        assert!(feedback_issue_url("t", "").is_err());
        let long = "标".repeat(TITLE_MAX + 1);
        assert!(feedback_issue_url(&long, "x").is_err());
        let long_body = "述".repeat(BODY_MAX + 1);
        assert!(feedback_issue_url("t", &long_body).is_err());
    }

    #[test]
    fn whitespace_inputs_are_trimmed() {
        let url = feedback_issue_url("  标题  ", "  正文  ").unwrap();
        let decoded = decode_query(&url);
        assert_eq!(
            decoded
                .iter()
                .find(|(k, _)| k == "title")
                .map(|(_, v)| v.as_str()),
            Some("标题")
        );
        assert!(
            decoded
                .iter()
                .find(|(k, _)| k == "body")
                .map(|(_, v)| v.as_str())
                .unwrap()
                .contains("正文"),
            "body 应含去空白后的描述"
        );
    }

    /// 解码 form_urlencoded 查询串（url crate 官方解析，防实现细节漂移）。
    fn decode_query(url: &str) -> Vec<(String, String)> {
        url::form_urlencoded::parse(url.rsplit('?').next().unwrap().as_bytes())
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }
}
