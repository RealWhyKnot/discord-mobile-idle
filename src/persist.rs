use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::controller::ADOPTABLE;
use crate::warning;

pub const START_FREE: usize = 3;
pub const START_WINDOW: f64 = 3600.0;
pub const START_BASE: u64 = 30;
pub const START_CAP: u64 = 1800;

pub fn start_delay(history: &[f64], now: f64) -> (u64, Vec<f64>) {
    let mut recent: Vec<f64> = history
        .iter()
        .copied()
        .filter(|t| (0.0..START_WINDOW).contains(&(now - t)))
        .collect();
    recent.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if recent.len() < START_FREE {
        return (0, recent);
    }
    let extra = (recent.len() - START_FREE) as u32;
    let delay = START_BASE.saturating_mul(2u64.saturating_pow(extra));
    (delay.min(START_CAP), recent)
}

pub struct StartLog {
    path: PathBuf,
}

impl StartLog {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        StartLog { path: path.into() }
    }

    pub fn load(&self) -> Vec<f64> {
        let Ok(text) = fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            match line.parse::<f64>() {
                Ok(v) => out.push(v),
                Err(_) => return Vec::new(),
            }
        }
        out
    }

    pub fn save(&self, history: &[f64]) {
        let text: String = history.iter().map(|t| format!("{:.0}\n", t)).collect();
        write_atomic(&self.path, &text);
    }

    pub fn forget(&self, stamp: f64) {
        let kept: Vec<f64> = self.load().into_iter().filter(|t| (t - stamp).abs() >= 1.0).collect();
        self.save(&kept);
    }
}

pub trait Marker {
    fn read(&self) -> Option<String>;
    fn write(&mut self, status: &str);
    fn clear(&mut self);
}

pub struct HoldMarker {
    path: PathBuf,
}

impl HoldMarker {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        HoldMarker { path: path.into() }
    }
}

impl Marker for HoldMarker {
    fn read(&self) -> Option<String> {
        let bytes = fs::read(&self.path).ok()?;
        if !bytes.is_ascii() {
            return None;
        }
        let status = String::from_utf8(bytes).ok()?.trim().to_string();
        ADOPTABLE.contains(&status.as_str()).then_some(status)
    }

    fn write(&mut self, status: &str) {
        write_atomic(&self.path, status);
    }

    fn clear(&mut self) {
        match fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => warning!("could not remove {}: {}", self.path.display(), e),
        }
    }
}

pub fn write_atomic(path: &Path, text: &str) {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = PathBuf::from(temp);
    let result = fs::write(&temp, text).and_then(|_| fs::rename(&temp, path));
    if let Err(e) = result {
        warning!("could not write {}: {}", path.display(), e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("idlebot-test-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_first_starts_in_an_hour_are_free() {
        assert_eq!(start_delay(&[100.0, 200.0], 300.0), (0, vec![100.0, 200.0]));
    }

    #[test]
    fn the_delay_doubles_past_the_free_starts() {
        let history: Vec<f64> = (0..5).map(|i| 1000.0 + i as f64).collect();
        let (delay, recent) = start_delay(&history, 2000.0);
        assert_eq!(delay, 120);
        assert_eq!(recent.len(), 5);
    }

    #[test]
    fn the_delay_is_capped() {
        let history: Vec<f64> = (0..40).map(|i| 1000.0 + i as f64).collect();
        assert_eq!(start_delay(&history, 2000.0).0, 1800);
        let history: Vec<f64> = (0..200).map(|i| 1000.0 + i as f64).collect();
        assert_eq!(start_delay(&history, 2000.0).0, 1800);
    }

    #[test]
    fn old_and_future_starts_are_dropped() {
        assert_eq!(start_delay(&[0.0, 5000.0, 9000.0], 4000.0), (0, vec![]));
    }

    #[test]
    fn the_start_log_round_trips_and_forgets() {
        let dir = scratch("starts");
        let log = StartLog::new(dir.join("starts"));
        log.save(&[10.0, 20.0, 30.0]);
        assert_eq!(log.load(), vec![10.0, 20.0, 30.0]);
        log.forget(20.4);
        assert_eq!(log.load(), vec![10.0, 30.0]);
    }

    #[test]
    fn a_missing_or_garbled_start_log_reads_empty() {
        let dir = scratch("garbled");
        let path = dir.join("starts");
        assert!(StartLog::new(&path).load().is_empty());
        fs::write(&path, "12\nnot a number\n").unwrap();
        assert!(StartLog::new(&path).load().is_empty());
    }

    #[test]
    fn the_hold_marker_round_trips_and_clears() {
        let dir = scratch("hold");
        let mut marker = HoldMarker::new(dir.join("hold"));
        marker.write("dnd");
        assert_eq!(marker.read().as_deref(), Some("dnd"));
        marker.clear();
        marker.clear();
        assert_eq!(marker.read(), None);
    }

    #[test]
    fn a_hold_marker_with_an_unexpected_value_is_ignored() {
        let dir = scratch("odd");
        let path = dir.join("hold");
        fs::write(&path, "idle").unwrap();
        assert_eq!(HoldMarker::new(&path).read(), None);
        fs::write(&path, [0xff, 0xfe]).unwrap();
        assert_eq!(HoldMarker::new(&path).read(), None);
    }

    #[test]
    fn a_marker_that_cannot_be_removed_only_warns() {
        let dir = scratch("dirmarker");
        let inner = dir.join("hold");
        fs::create_dir(&inner).unwrap();
        log::take();
        HoldMarker::new(&inner).clear();
        assert!(log::take().iter().any(|(_, m)| m.contains("could not remove")));
    }

    #[test]
    fn an_unwritable_path_only_warns() {
        let dir = scratch("unwritable");
        log::take();
        write_atomic(&dir.join("missing").join("file"), "x");
        assert!(log::take().iter().any(|(_, m)| m.contains("could not write")));
        assert!(!dir.join("missing").exists());
    }
}
