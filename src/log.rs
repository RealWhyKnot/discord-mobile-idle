use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Warning,
    Error,
}

impl Level {
    fn name(self) -> &'static str {
        match self {
            Level::Info => "INFO",
            Level::Warning => "WARNING",
            Level::Error => "ERROR",
        }
    }
}

#[cfg(test)]
thread_local! {
    pub static CAPTURED: std::cell::RefCell<Vec<(Level, String)>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub fn take() -> Vec<(Level, String)> {
    CAPTURED.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

pub fn emit(level: Level, message: &str) {
    #[cfg(test)]
    {
        CAPTURED.with(|c| c.borrow_mut().push((level, message.to_string())));
        return;
    }
    #[allow(unreachable_code)]
    let line = format!("{} {} {}\n", timestamp(SystemTime::now()), level.name(), message);
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(line.as_bytes());
    let _ = out.flush();
}

pub fn timestamp(now: SystemTime) -> String {
    let since = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs();
    let (year, month, day) = civil_from_days((secs / 86_400) as i64);
    let rest = secs % 86_400;
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02},{:03}",
        year,
        month,
        day,
        rest / 3600,
        rest % 3600 / 60,
        rest % 60,
        since.subsec_millis()
    )
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => { $crate::log::emit($crate::log::Level::Info, &format!($($arg)*)) };
}

#[macro_export]
macro_rules! warning {
    ($($arg:tt)*) => { $crate::log::emit($crate::log::Level::Warning, &format!($($arg)*)) };
}

#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => { $crate::log::emit($crate::log::Level::Error, &format!($($arg)*)) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn formats_like_python_asctime() {
        let at = UNIX_EPOCH + Duration::from_millis(1_791_945_369_123);
        assert_eq!(timestamp(at), "2026-10-14 02:36:09,123");
    }

    #[test]
    fn handles_leap_days_and_the_epoch() {
        assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01 00:00:00,000");
        let leap = UNIX_EPOCH + Duration::from_secs(1_709_164_800);
        assert_eq!(timestamp(leap), "2024-02-29 00:00:00,000");
    }
}
