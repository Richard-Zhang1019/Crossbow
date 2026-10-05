//! 订阅自动更新调度器（M1.1）。
//!
//! 每 30s 扫描一次：远程 Profile 且 `update_interval_min > 0`、到达下次
//! 更新时间者执行刷新。刷新复用命令层同一套逻辑（失败保留旧内容），
//! 失败后按指数退避重试（15min 起，×2，封顶 6h）。

use std::collections::HashMap;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

use crossbow_core::subscription::now_secs;

/// 常规 tick 间隔。
const TICK_SECS: u64 = 30;
/// 失败重试的基础退避。
const BACKOFF_BASE_SECS: u64 = 15 * 60;
/// 退避封顶。
const BACKOFF_MAX_SECS: u64 = 6 * 3600;

/// 失败重试退避：15min × 2^fails，封顶 6h。纯函数，便于测试。
pub fn backoff_delay_secs(consecutive_fails: u32) -> u64 {
    let shift = consecutive_fails.saturating_sub(1).min(16);
    BACKOFF_BASE_SECS
        .saturating_mul(1u64 << shift)
        .min(BACKOFF_MAX_SECS)
}

/// 调度器内存态：连续失败次数与下一次重试时间（仅失败时使用）。
#[derive(Default)]
struct RetryState {
    fails: HashMap<String, (u32, u64)>,
}

/// 判定某 Profile 本次 tick 是否到期。
fn is_due(profile: &crossbow_core::Profile, retry: Option<&(u32, u64)>, now: u64) -> bool {
    if let Some(&(fails, retry_at)) = retry {
        // 处于失败重试期：到达重试点才算到期
        return now >= retry_at && {
            let _ = fails;
            true
        };
    }
    let Some(interval) = NonZeroInterval::new(profile.update_interval_min) else {
        return false;
    };
    // 从未更新过的旧数据：立即安排一次。
    let Some(last) = profile.last_updated else {
        return true;
    };
    now >= last.saturating_add(interval.secs())
}

/// 非零间隔的轻封装（避免到处判 0）。
struct NonZeroInterval(u32);

impl NonZeroInterval {
    fn new(min: u32) -> Option<Self> {
        (min > 0).then_some(Self(min))
    }
    fn secs(&self) -> u64 {
        self.0 as u64 * 60
    }
}

/// 后台线程入口：随应用启动，随进程退出结束。
pub fn spawn(app: AppHandle) {
    std::thread::spawn(move || {
        let mut retry: RetryState = RetryState::default();
        loop {
            std::thread::sleep(Duration::from_secs(TICK_SECS));
            tick(&app, &mut retry);
        }
    });
}

fn tick(app: &AppHandle, retry: &mut RetryState) {
    let state = app.state::<crate::AppState>();
    let now = now_secs();

    // 1) 快照到期列表（短锁，不做网络）
    let due: Vec<String> = {
        let store = state.store.lock().unwrap();
        store
            .data()
            .profiles
            .iter()
            .filter(|p| p.kind == crossbow_core::ProfileKind::Remote)
            .filter(|p| is_due(p, retry.fails.get(&p.id), now))
            .map(|p| p.id.clone())
            .collect()
    };
    if due.is_empty() {
        return;
    }

    for id in due {
        // 2) 逐个刷新；网络在 refresh 内部于持锁外完成
        let result = crate::refresh_profile_blocking(&state, &id);
        match result {
            Ok(_) => {
                retry.fails.remove(&id);
            }
            Err(e) => {
                let entry = retry.fails.entry(id.clone()).or_insert((0, 0));
                entry.0 = entry.0.saturating_add(1);
                let delay = backoff_delay_secs(entry.0);
                entry.1 = now + delay;
                eprintln!(
                    "auto-update {id} failed ({}): {e}; retry in {delay}s",
                    entry.0
                );
            }
        }
        // 3) 通知前端刷新（无论成败——失败也要更新卡片上的结果展示）
        let _ = app.emit("profile://updated", id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbow_core::{Profile, ProfileKind};

    fn remote_profile(interval_min: u32, last_updated: Option<u64>) -> Profile {
        Profile {
            id: "p1".into(),
            name: "n".into(),
            kind: ProfileKind::Remote,
            url: Some("https://x".into()),
            content: String::new(),
            update_interval_min: interval_min,
            override_ids: vec![],
            last_updated,
            traffic: None,
            last_update: None,
        }
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff_delay_secs(1), 900);
        assert_eq!(backoff_delay_secs(2), 1800);
        assert_eq!(backoff_delay_secs(3), 3600);
        assert_eq!(backoff_delay_secs(4), 7200);
        assert_eq!(backoff_delay_secs(5), 14400);
        assert_eq!(backoff_delay_secs(6), 21600);
        // 封顶 6h
        assert_eq!(backoff_delay_secs(10), 21600);
        assert_eq!(backoff_delay_secs(100), 21600);
    }

    #[test]
    fn zero_interval_never_due() {
        let p = remote_profile(0, Some(1));
        assert!(!is_due(&p, None, 10_000_000));
    }

    #[test]
    fn due_after_interval_elapsed() {
        let p = remote_profile(60, Some(1000)); // 1 小时间隔
        assert!(!is_due(&p, None, 1000 + 59 * 60));
        assert!(is_due(&p, None, 1000 + 61 * 60));
    }

    #[test]
    fn none_last_updated_is_due() {
        let p = remote_profile(60, None);
        assert!(is_due(&p, None, 0));
    }

    #[test]
    fn retry_state_overrides_interval() {
        let p = remote_profile(60, Some(1000));
        // 间隔未到，但失败重试点已到
        assert!(is_due(&p, Some(&(1, 2000)), 2000));
        // 重试点未到：即使间隔名义上到了也不刷新（退避优先）
        assert!(!is_due(&p, Some(&(1, 9000)), 8000));
    }
}
