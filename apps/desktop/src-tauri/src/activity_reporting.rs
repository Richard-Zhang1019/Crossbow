//! Anonymous desktop activity reporting. Keep preferences and daily state outside profile backups.

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
    #[serde(default)]
    proxy_attempts: usize,
    #[serde(default)]
    next_proxy: u64,
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
        let state = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => DailyState {
                enabled: true,
                ..DailyState::default()
            },
            // An unreadable preference must not override a possible opt-out.
            Err(_) => DailyState::default(),
        };
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

        let proxy =
            if next.direct_attempts > 0 && retry_due(next.proxy_attempts, next.next_proxy, now) {
                proxy_port()
            } else {
                None
            };
        let (route, attempts, next_attempt) = if let Some(port) = proxy {
            (
                Route::Proxy(port),
                &mut next.proxy_attempts,
                &mut next.next_proxy,
            )
        } else if retry_due(next.direct_attempts, next.next_direct, now) {
            (
                Route::Direct,
                &mut next.direct_attempts,
                &mut next.next_direct,
            )
        } else {
            return None;
        };
        if let Some(delay) = RETRY_DELAYS.get(*attempts) {
            *next_attempt = now.saturating_add(*delay);
        }
        *attempts += 1;

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

fn retry_due(attempts: usize, next_attempt: u64, now: u64) -> bool {
    attempts <= RETRY_DELAYS.len() && now >= next_attempt
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
    fn reporting_defaults_on_and_an_explicit_opt_out_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let reporter = Reporter::open(dir.path());
        assert!(reporter.enabled());
        reporter.set_enabled(false).unwrap();
        drop(reporter);

        let reporter = Reporter::open(dir.path());
        assert!(!reporter.enabled());
        assert!(reporter
            .reserve(1_800_000_000, || panic!("reporting disabled"))
            .is_none());
    }

    #[test]
    fn proxy_can_retry_five_times_after_the_direct_budget_is_spent() {
        let dir = tempfile::tempdir().unwrap();
        let mut now = 1_800_000_000;
        let reporter = Reporter::open(dir.path());
        let first = reporter
            .reserve(now, || panic!("always start directly"))
            .unwrap();
        assert_eq!(first.route, Route::Direct);

        for delay in RETRY_DELAYS {
            now += delay;
            assert!(reporter.reserve(now - 1, || None).is_none());
            let retry = reporter.reserve(now, || None).unwrap();
            assert_eq!(retry.route, Route::Direct);
            assert_eq!(retry.daily_id, first.daily_id);
        }
        now += 10;
        assert!(reporter.reserve(now, || None).is_none());
        let proxy = reporter.reserve(now, || Some(7898)).unwrap();
        assert_eq!(proxy.route, Route::Proxy(7898));
        assert_eq!(proxy.daily_id, first.daily_id);
        drop(reporter);

        let reporter = Reporter::open(dir.path());
        for delay in RETRY_DELAYS {
            now += delay;
            assert!(reporter.reserve(now - 1, || Some(7898)).is_none());
            assert!(reporter.reserve(now, || None).is_none());
            let retry = reporter.reserve(now, || Some(7898)).unwrap();
            assert_eq!(retry.route, Route::Proxy(7898));
            assert_eq!(retry.daily_id, first.daily_id);
        }
        assert!(reporter.reserve(now + 3600, || Some(7898)).is_none());
    }

    #[test]
    fn success_survives_restart_and_the_next_beijing_day_uses_a_new_id() {
        let dir = tempfile::tempdir().unwrap();
        let midnight = 21_000 * 86_400 - 8 * 3600;
        let reporter = Reporter::open(dir.path());
        let first = reporter.reserve(midnight - 90, || None).unwrap();
        let proxy = reporter.reserve(midnight - 80, || Some(7898)).unwrap();
        assert_eq!(proxy.route, Route::Proxy(7898));
        let retry = reporter.reserve(midnight - 20, || Some(7898)).unwrap();
        assert_eq!(retry.route, Route::Proxy(7898));
        reporter.record_success(&retry);
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
