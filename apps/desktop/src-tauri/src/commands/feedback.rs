//! #544：用户反馈问题——按钮直达 GitHub 的 issue 模板选择页。
//!
//! 不经过自建后端、不做表单中转：用户在 GitHub 上按模板填写并提交。
//! 无存储面、无隐私新增面（打开动作本身不携带任何用户数据）。

/// GitHub issues/new 的模板选择页（用户自选 bug / feature 模板）。
const FEEDBACK_URL: &str = "https://github.com/Richard-Zhang1019/Crossbow/issues/new/choose";

/// 白名单：本命令只打开固定页面——未来调用方变化也不能引入任意 URL 打开。
#[tauri::command]
pub(crate) async fn open_feedback_page() -> Result<(), String> {
    super::monitor::open_in_system_browser(FEEDBACK_URL, "Could not open feedback page").await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_url_points_at_issue_template_picker() {
        assert_eq!(
            FEEDBACK_URL,
            "https://github.com/Richard-Zhang1019/Crossbow/issues/new/choose"
        );
    }
}
