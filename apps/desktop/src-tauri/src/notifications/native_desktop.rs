//! #528：托盘态（主窗口关闭/最小化）通知走系统级通知中心。
//! macOS = UNUserNotificationCenter（mac-notification-sys），Windows = Toast
//! （tauri-winrt-notification）。批次内含 Error 类通知时伴随系统默认
//! 提示音。批次内容（通知 id 集合）不变时不重复投递；发送失败返回
//! false，调用方回退浮窗路径。仅主线程调用（route() 在 AppKit 主循环）。

use std::collections::BTreeSet;
use std::sync::Mutex;

use crate::notifications::{Kind, Notice};

/// 上一次已投递批次的通知 id 集合：批次未变则不重复弹系统横幅。
static LAST_BATCH: Mutex<Option<BTreeSet<String>>> = Mutex::new(None);

/// Error 类通知伴随系统默认提示音；其余类别静默横幅。
/// 音效名按平台取系统常量：macOS 的 NSUserNotification.soundName 需要
/// `NSUserNotificationDefaultSoundName` 字面量（"Default" 不是有效音效名，
/// 静默）；Windows 的 Sound::from_str 识别 "Default"（toast 省略 audio
/// 元素 = 默认提示音）。
#[cfg(target_os = "macos")]
const DEFAULT_SOUND: &str = "NSUserNotificationDefaultSoundName";
#[cfg(target_os = "windows")]
const DEFAULT_SOUND: &str = "Default";

fn sound(notices: &[Notice]) -> Option<&'static str> {
    notices
        .iter()
        .any(|n| n.kind == Kind::Error)
        .then_some(DEFAULT_SOUND)
}

fn batch_key(notices: &[Notice]) -> BTreeSet<String> {
    notices.iter().map(|n| n.id.clone()).collect()
}

/// 托盘态投递：批次内容变化时经系统通知中心发一条汇总横幅。返回 true
/// 表示已通过系统通知投递（调用方跳过浮窗），false 表示发送失败
/// （调用方回退浮窗）。
pub(crate) fn update(notices: &[Notice], english: bool) -> bool {
    let key = batch_key(notices);
    let mut last = LAST_BATCH.lock().unwrap_or_else(|e| e.into_inner());
    if last.as_ref() == Some(&key) {
        // 批次未变：不重复弹系统横幅，也不回退浮窗（通知列表仍是完整交互面）。
        return true;
    }
    let (title, body) = if english {
        (
            "New Crossbow notifications",
            "Open Crossbow to view details",
        )
    } else {
        ("Crossbow 有新通知", "打开 Crossbow 查看详情")
    };
    let mut notification = notify_rust::Notification::new();
    notification.summary(title).body(body);
    if let Some(sound) = sound(notices) {
        notification.sound_name(sound);
    }
    let delivered = notification.show().is_ok();
    if delivered {
        *last = Some(key);
    }
    delivered
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(id: &str, kind: Kind) -> Notice {
        let mut notice = Notice::new(kind, "code", "test-source");
        notice.id = id.to_string();
        notice
    }

    /// Error 类通知伴随系统默认提示音；其余类别静默。
    #[test]
    fn sound_follows_error_kind_only() {
        let errors = [notice("a", Kind::Error)];
        assert_eq!(sound(&errors), Some(DEFAULT_SOUND));
        let mixed = [notice("a", Kind::Error), notice("b", Kind::Success)];
        assert_eq!(sound(&mixed), Some(DEFAULT_SOUND));
        let silent = [notice("a", Kind::Success), notice("b", Kind::Info)];
        assert_eq!(sound(&silent), None);
    }

    /// 批次键 = 通知 id 集合：同内容去重，任一变化都触发重新投递。
    #[test]
    fn batch_key_tracks_notice_identity() {
        let a = [notice("a", Kind::Error)];
        let same = [notice("a", Kind::Error)];
        let added = [notice("a", Kind::Error), notice("b", Kind::Warning)];
        assert_eq!(batch_key(&a), batch_key(&same));
        assert_ne!(batch_key(&a), batch_key(&added));
    }
}
