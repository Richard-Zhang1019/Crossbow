//! Voluntary desktop activity reporting. Keep consent and daily state outside profile backups.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use crossbow_core::subscription::now_secs;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

use crate::{AppState, CoreStatus};

const ENDPOINT: &str = "https://crossbow-metrics.aaaayu1019.workers.dev/api/v1/active";
const RETRY_DELAYS: [u64; 5] = [60, 120, 300, 900, 1800];

fn beijing_day(now: u64) -> u64 {
    now.saturating_add(8 * 3600) / 86_400
}

#[derive(Clone, Default, Deserialize, Serialize)]
struct DailyState {
    enabled: bool,
    day: u64,
    daily_id: String,
    direct_attempts: usize,
    next_direct: u64,
    proxy_attempted: bool,
    succeeded: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Route {
    Direct,
    Proxy(u16),
}

struct Attempt {
    day: u64,
    daily_id: String,
    route: Route,
}

pub(crate) struct Reporter {
    path: PathBuf,
    state: Mutex<DailyState>,
}

impl Reporter {
    pub(crate) fn open(data_dir: &Path) -> Self {
        let path = data_dir.join("activity-reporting.json");
        let state = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            path,
            state: Mutex::new(state),
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.state.lock().unwrap().enabled
    }

    pub(crate) fn set_enabled(&self, enabled: bool) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        if state.enabled != enabled {
            let next = DailyState {
                enabled,
                ..state.clone()
            };
            self.save(&next)?;
            *state = next;
        }
        Ok(())
    }

    fn save(&self, state: &DailyState) -> io::Result<()> {
        let tmp = self.path.with_extension("tmp");
        fs::write(&tmp, serde_json::to_vec(state)?)?;
        fs::rename(tmp, &self.path)
    }

    fn reserve(&self, now: u64, proxy_port: impl FnOnce() -> Option<u16>) -> Option<Attempt> {
        let mut state = self.state.lock().unwrap();
        let day = beijing_day(now);
        if !state.enabled || day < state.day || (day == state.day && state.succeeded) {
            return None;
        }

        let mut next = if day > state.day || state.daily_id.is_empty() {
            DailyState {
                enabled: true,
                day,
                daily_id: Uuid::new_v4().simple().to_string(),
                ..DailyState::default()
            }
        } else {
            state.clone()
        };

        let proxy = if next.direct_attempts > 0 && !next.proxy_attempted {
            proxy_port()
        } else {
            None
        };
        let route = if let Some(port) = proxy {
            next.proxy_attempted = true;
            Route::Proxy(port)
        } else if next.direct_attempts <= RETRY_DELAYS.len() && now >= next.next_direct {
            if let Some(delay) = RETRY_DELAYS.get(next.direct_attempts) {
                next.next_direct = now.saturating_add(*delay);
            }
            next.direct_attempts += 1;
            Route::Direct
        } else {
            return None;
        };

        // Save the ID and retry budget before the request, including across restarts.
        self.save(&next).ok()?;
        let attempt = Attempt {
            day,
            daily_id: next.daily_id.clone(),
            route,
        };
        *state = next;
        Some(attempt)
    }

    fn record_success(&self, attempt: &Attempt) {
        let mut state = self.state.lock().unwrap();
        if state.day == attempt.day && state.daily_id == attempt.daily_id {
            state.succeeded = true;
            // Keep success in memory if saving fails. The server also deduplicates.
            let _ = self.save(&state);
        }
    }
}

fn mixed_port_when_proxy_ready(state: &AppState) -> Option<u16> {
    if state.core().status() != CoreStatus::Running {
        return None;
    }
    let (tun_enabled, mixed_port) = {
        let store = state.store.lock().unwrap();
        let engine = &store.data().engine;
        (engine.tun_enable, engine.mixed_port)
    };
    let sysproxy = state.sysproxy.status();
    (tun_enabled || (sysproxy.enabled && sysproxy.port == mixed_port)).then_some(mixed_port)
}

fn send(attempt: &Attempt, version: &str) -> bool {
    let mut builder = ureq::AgentBuilder::new()
        .try_proxy_from_env(false)
        .timeout(Duration::from_secs(5))
        .redirects(0);
    if let Route::Proxy(port) = attempt.route {
        let Ok(proxy) = ureq::Proxy::new(format!("http://127.0.0.1:{port}")) else {
            return false;
        };
        builder = builder.proxy(proxy);
    }
    let payload = serde_json::json!({
        "day": attempt.day,
        "daily_id": attempt.daily_id,
        "platform": std::env::consts::OS,
        "version": version,
    });
    builder
        .build()
        .post(ENDPOINT)
        .set("Content-Type", "application/json")
        .send_string(&payload.to_string())
        .is_ok_and(|response| response.status() == 204)
}

pub(crate) fn spawn(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        while state.exit.ensure_open().is_ok() {
            if let Some(attempt) = state
                .activity_reporting
                .reserve(now_secs(), || mixed_port_when_proxy_ready(&state))
            {
                if state.activity_reporting.enabled()
                    && state.exit.ensure_open().is_ok()
                    && send(&attempt, &app.package_info().version.to_string())
                {
                    state.activity_reporting.record_success(&attempt);
                }
            }
            std::thread::sleep(Duration::from_secs(10));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_five_direct_retries_and_only_one_proxy_retry_even_across_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let now = 1_800_000_000;
        let reporter = Reporter::open(dir.path());
        assert!(reporter.reserve(now, || panic!("not opted in")).is_none());

        reporter.set_enabled(true).unwrap();
        let first = reporter
            .reserve(now, || panic!("always start directly"))
            .unwrap();
        assert_eq!(first.route, Route::Direct);
        drop(reporter);

        let reporter = Reporter::open(dir.path());
        let proxy = reporter.reserve(now + 10, || Some(7898)).unwrap();
        assert_eq!(proxy.route, Route::Proxy(7898));
        assert_eq!(proxy.daily_id, first.daily_id);

        let mut next = now;
        for delay in RETRY_DELAYS {
            next += delay;
            assert!(reporter.reserve(next - 1, || Some(7898)).is_none());
            let retry = reporter.reserve(next, || Some(7898)).unwrap();
            assert_eq!(retry.route, Route::Direct);
            assert_eq!(retry.daily_id, first.daily_id);
        }
        reporter.set_enabled(false).unwrap();
        reporter.set_enabled(true).unwrap();
        assert!(reporter.reserve(next + 3600, || Some(7898)).is_none());
    }

    #[test]
    fn success_survives_restart_and_the_next_beijing_day_uses_a_new_id() {
        let dir = tempfile::tempdir().unwrap();
        let midnight = 21_000 * 86_400 - 8 * 3600;
        let reporter = Reporter::open(dir.path());
        reporter.set_enabled(true).unwrap();
        let first = reporter.reserve(midnight - 30, || None).unwrap();
        reporter.record_success(&first);
        drop(reporter);

        let reporter = Reporter::open(dir.path());
        assert!(reporter.reserve(midnight - 1, || Some(7898)).is_none());
        let next = reporter.reserve(midnight, || Some(7898)).unwrap();
        assert_eq!(next.day, first.day + 1);
        assert_ne!(next.daily_id, first.daily_id);
        assert_eq!(next.route, Route::Direct);
        assert!(reporter.reserve(midnight - 1, || Some(7898)).is_none());
    }
}
