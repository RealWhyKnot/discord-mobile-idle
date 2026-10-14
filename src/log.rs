use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
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

pub struct Rotating {
    path: PathBuf,
    max_bytes: u64,
    backups: u32,
    file: Option<File>,
}

impl Rotating {
    pub fn new(path: PathBuf, max_bytes: u64, backups: u32) -> Self {
        Rotating {
            path,
            max_bytes,
            backups,
            file: None,
        }
    }

    pub fn write(&mut self, line: &str) {
        let size = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        if size > 0 && size + line.len() as u64 >= self.max_bytes {
            self.roll();
        }
        if self.file.is_none() {
            self.file = OpenOptions::new().create(true).append(true).open(&self.path).ok();
        }
        if let Some(file) = &mut self.file {
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
        }
    }

    fn roll(&mut self) {
        self.file = None;
        for i in (1..self.backups).rev() {
            let from = numbered(&self.path, i);
            if from.exists() {
                let to = numbered(&self.path, i + 1);
                let _ = std::fs::remove_file(&to);
                let _ = std::fs::rename(&from, &to);
            }
        }
        if self.backups > 0 {
            let first = numbered(&self.path, 1);
            let _ = std::fs::remove_file(&first);
            let _ = std::fs::rename(&self.path, &first);
        } else {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn numbered(path: &Path, i: u32) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{}", i));
    PathBuf::from(name)
}

struct Sink {
    file: Rotating,
    stamp: fn() -> String,
}

static SINK: Mutex<Option<Sink>> = Mutex::new(None);

const EOL: &str = if cfg!(windows) { "\r\n" } else { "\n" };

pub fn to_file(file: Rotating, stamp: fn() -> String) {
    *SINK.lock().unwrap_or_else(|e| e.into_inner()) = Some(Sink { file, stamp });
}

pub fn emit(level: Level, message: &str) {
    #[cfg(test)]
    {
        CAPTURED.with(|c| c.borrow_mut().push((level, message.to_string())));
        return;
    }
    #[allow(unreachable_code)]
    {
        let mut sink = SINK.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(sink) = sink.as_mut() {
            let line = format!("{} {} {}{}", (sink.stamp)(), level.name(), message, EOL);
            sink.file.write(&line);
            return;
        }
    }
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

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("idlebot-log-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    }

    #[test]
    fn rotates_like_python_and_keeps_three_backups() {
        let dir = scratch("rotate");
        let path = dir.join("idlebot.log");
        let mut log = Rotating::new(path.clone(), 10, 3);
        for line in ["aaaa\n", "bbbb\n", "cccc\n", "dddd\n", "eeee\n", "ffff\n"] {
            log.write(line);
        }
        assert_eq!(read(&path), "ffff\n");
        assert_eq!(read(&numbered(&path, 1)), "eeee\n");
        assert_eq!(read(&numbered(&path, 2)), "dddd\n");
        assert_eq!(read(&numbered(&path, 3)), "cccc\n");
        assert!(!numbered(&path, 4).exists());
        drop(log);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn appends_until_the_limit_and_never_rolls_an_empty_file() {
        let dir = scratch("append");
        let path = dir.join("idlebot.log");
        let mut log = Rotating::new(path.clone(), 10, 3);
        log.write("this line is longer than the limit\n");
        assert!(!numbered(&path, 1).exists());
        let mut small = Rotating::new(dir.join("small.log"), 100, 3);
        small.write("one\n");
        small.write("two\n");
        assert_eq!(read(&dir.join("small.log")), "one\ntwo\n");
        let mut none = Rotating::new(dir.join("none.log"), 5, 0);
        none.write("1234\n");
        none.write("5678\n");
        assert_eq!(read(&dir.join("none.log")), "5678\n");
        drop((log, small, none));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
