//! 日志本地落盘（#29）：按天分文件 + 保留天数自动清理。
//!
//! 设计：`data_dir/logs/YYYY-MM-DD.log`，每行 `HH:MM:SS [LEVEL] payload`；
//! 写入端 `DailyWriter` 由 logs 订阅线程独占（免锁），跨天自动换文件；
//! 清理 `sweep` 按文件名日期计算“距今天数”，超期即删。全部 std::fs，
//! 无平台 API——Windows/iOS 复用。

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::platform::fs_perms;

pub fn logs_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("logs")
}

/// 本地日期 (年, 月, 日)。unix 用 localtime_r，Windows 用 localtime_s，
/// 失败退回 UTC（保留期边界最多偏移数小时，可接受）。
fn today_ymd() -> (i32, u32, u32, String) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // 本地时区：仅 unix（libc 已是依赖）。Windows 无 libc 依赖，
    // 直接用 UTC 命名（保留期边界偏移数小时，可接受；wintun 移植时再补）。
    #[cfg(unix)]
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let t: libc::time_t = secs as libc::time_t;
        if !libc::localtime_r(&t, &mut tm).is_null() {
            let time = format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec);
            return (
                tm.tm_year + 1900,
                (tm.tm_mon + 1) as u32,
                tm.tm_mday as u32,
                time,
            );
        }
    }
    // UTC 回退
    let days = secs.div_euclid(86400);
    let (y, m, d) = epoch_days_to_ymd(days);
    let rem = secs.rem_euclid(86400);
    let time = format!("{:02}:{:02}:{:02}", rem / 3600, (rem % 3600) / 60, rem % 60);
    (y, m, d, time)
}

/// 民用日 → 自 1970-01-01 的天数（Howard Hinnant 算法，与 ws_bridge 同源）。
fn ymd_to_epoch_days(y: i32, m: u32, d: u32) -> i64 {
    let (y, m, d) = (y as i64, m as i64, d as i64);
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = m - 3;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// 自 1970-01-01 天数 → 民用日。
fn epoch_days_to_ymd(days: i64) -> (i32, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = (y + i64::from(m <= 2)) as i32;
    (y, m, d)
}

/// 按天追加写入器：跨天自动换文件。由单一订阅线程独占，免锁。
pub struct DailyWriter {
    dir: PathBuf,
    day: String,
    file: Option<BufWriter<File>>,
}

impl DailyWriter {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            day: String::new(),
            file: None,
        }
    }

    /// 追加一行日志；内部维护“今天”的文件句柄，跨天自动切换。
    pub fn write(&mut self, level: &str, payload: &str) {
        let (y, m, d, time) = today_ymd();
        let day = format!("{y:04}-{m:02}-{d:02}");
        if self.file.is_none() || self.day != day {
            if std::fs::create_dir_all(&self.dir).is_err() {
                return;
            }
            let path = self.dir.join(format!("{day}.log"));
            self.file = fs_perms::open_private_append(&path)
                .ok()
                .map(BufWriter::new);
            self.day = day;
        }
        if let Some(f) = self.file.as_mut() {
            let _ = writeln!(f, "{time} [{level}] {payload}");
            let _ = f.flush();
        }
    }
}

/// 清理超期日志：`today - 文件日期 >= retention_days` 即删除。
/// 返回删除的文件数。
pub fn sweep(dir: &Path, retention_days: u32) -> usize {
    let (y, m, d, _) = today_ymd();
    let today = ymd_to_epoch_days(y, m, d);
    let mut removed = 0;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(stem) = name.strip_suffix(".log") else {
            continue;
        };
        let Some((y, m, dd)) = parse_ymd(stem) else {
            continue;
        };
        let path = entry.path();
        if ymd_to_epoch_days(y, m, dd) + retention_days as i64 > today {
            fs_perms::ensure_private(&path);
        } else if std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

fn parse_ymd(s: &str) -> Option<(i32, u32, u32)> {
    let mut it = s.split('-');
    let y = it.next()?.parse::<i32>().ok()?;
    let m = it.next()?.parse::<u32>().ok()?;
    let d = it.next()?.parse::<u32>().ok()?;
    it.next().is_none().then_some((y, m, d))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_creates_daily_file_and_writes_lines() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = DailyWriter::new(dir.path().to_path_buf());
        w.write("info", "hello");
        w.write("warning", "world");
        let (y, m, d, _) = today_ymd();
        let file = dir.path().join(format!("{y:04}-{m:02}-{d:02}.log"));
        let content = std::fs::read_to_string(&file).unwrap();
        assert!(content.contains("[info] hello"));
        assert!(content.contains("[warning] world"));
    }

    #[test]
    fn sweep_deletes_only_expired_files() {
        let dir = tempfile::tempdir().unwrap();
        // 造两个“过期”与一个“今天”的文件
        std::fs::write(dir.path().join("2026-01-01.log"), "old").unwrap();
        std::fs::write(dir.path().join("2026-01-02.log"), "older").unwrap();
        let (y, m, d, _) = today_ymd();
        std::fs::write(
            dir.path().join(format!("{y:04}-{m:02}-{d:02}.log")),
            "today",
        )
        .unwrap();
        let removed = sweep(dir.path(), 7);
        assert_eq!(removed, 2);
        assert!(!dir.path().join("2026-01-01.log").exists());
        assert!(dir
            .path()
            .join(format!("{y:04}-{m:02}-{d:02}.log"))
            .exists());
    }

    #[test]
    fn ymd_roundtrip() {
        let days = ymd_to_epoch_days(2026, 10, 23);
        assert_eq!(epoch_days_to_ymd(days), (2026, 10, 23));
    }
}
