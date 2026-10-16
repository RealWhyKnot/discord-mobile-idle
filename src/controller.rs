use std::collections::BTreeSet;
use std::rc::Rc;

pub const IDLE: &str = "idle";
pub const ONLINE: &str = "online";
pub const DND: &str = "dnd";
pub const INVISIBLE: &str = "invisible";
pub const OFFLINE: &str = "offline";

pub const ALL_STATUSES: [&str; 5] = [ONLINE, IDLE, DND, INVISIBLE, OFFLINE];
pub const ADOPTABLE: [&str; 2] = [ONLINE, DND];

pub type Clock = Rc<dyn Fn() -> f64>;

pub fn seconds() -> f64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_secs_f64() + 1.0
}

pub fn monotonic() -> Clock {
    Rc::new(seconds)
}

pub struct StatusController {
    pub idle_status: &'static str,
    pub default_restore: String,
    pub managed: BTreeSet<String>,
    pub grace_seconds: f64,
    pub clock: Clock,
    pub on_mobile: bool,
    pub saved: Option<String>,
    pub holding: bool,
    pub grace_until: f64,
    pub adopt: bool,
}

impl StatusController {
    pub fn new(default_restore: &str, managed: BTreeSet<String>, grace_seconds: f64, clock: Clock) -> Self {
        StatusController {
            idle_status: IDLE,
            default_restore: default_restore.to_string(),
            managed,
            grace_seconds,
            clock,
            on_mobile: false,
            saved: None,
            holding: false,
            grace_until: 0.0,
            adopt: true,
        }
    }

    pub fn evaluate(&mut self, is_on_mobile: bool, current: &str) -> Option<String> {
        if self.holding && !self.in_grace() && current != self.idle_status {
            self.release();
            return None;
        }
        if is_on_mobile == self.on_mobile {
            return None;
        }
        self.on_mobile = is_on_mobile;
        if is_on_mobile {
            self.take(current)
        } else {
            self.hand_back(current)
        }
    }

    fn in_grace(&self) -> bool {
        (self.clock)() < self.grace_until
    }

    fn take(&mut self, current: &str) -> Option<String> {
        let adopting = current == self.idle_status;
        if (adopting && !self.adopt) || (!adopting && !self.managed.contains(current)) {
            return None;
        }
        self.saved = if adopting { None } else { Some(current.to_string()) };
        self.holding = true;
        self.grace_until = (self.clock)() + self.grace_seconds;
        Some(self.idle_status.to_string())
    }

    fn hand_back(&mut self, current: &str) -> Option<String> {
        if !self.holding {
            return None;
        }
        let target = self.restore_target();
        self.release();
        if current != self.idle_status {
            return None;
        }
        Some(target)
    }

    pub fn restore_target(&self) -> String {
        self.saved.clone().unwrap_or_else(|| self.default_restore.clone())
    }

    pub fn resume_hold(&mut self, saved: &str) {
        self.on_mobile = true;
        self.holding = true;
        self.saved = Some(saved.to_string());
        self.grace_until = (self.clock)() + self.grace_seconds;
    }

    pub fn release(&mut self) {
        self.holding = false;
        self.saved = None;
        self.grace_until = 0.0;
    }

    pub fn stand_down(&mut self) {
        self.step_aside(false);
    }

    pub fn step_aside(&mut self, is_on_mobile: bool) {
        self.release();
        self.on_mobile = is_on_mobile;
    }
}

pub struct Debouncer {
    pub on_polls: u32,
    pub off_polls: u32,
    pub stable: bool,
    pub pending: bool,
    pub count: u32,
}

impl Debouncer {
    pub fn new(on_polls: u32, off_polls: u32) -> Self {
        Self::with_initial(on_polls, off_polls, false)
    }

    pub fn with_initial(on_polls: u32, off_polls: u32, initial: bool) -> Self {
        Debouncer {
            on_polls,
            off_polls,
            stable: initial,
            pending: initial,
            count: 0,
        }
    }

    pub fn update(&mut self, reading: bool) -> bool {
        if reading == self.stable {
            self.pending = reading;
            self.count = 0;
            return self.stable;
        }
        if reading != self.pending {
            self.pending = reading;
            self.count = 1;
        } else {
            self.count += 1;
        }
        let needed = if reading { self.on_polls } else { self.off_polls };
        if self.count >= needed {
            self.stable = reading;
            self.count = 0;
        }
        self.stable
    }

    pub fn reset(&mut self) {
        self.pending = self.stable;
        self.count = 0;
    }

    pub fn force(&mut self, value: bool) {
        self.stable = value;
        self.reset();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::Cell;

    #[derive(Clone)]
    pub struct FakeClock(pub Rc<Cell<f64>>);

    impl FakeClock {
        pub fn new() -> Self {
            FakeClock(Rc::new(Cell::new(1000.0)))
        }
        pub fn advance(&self, seconds: f64) {
            self.0.set(self.0.get() + seconds);
        }
        pub fn clock(&self) -> Clock {
            let cell = self.0.clone();
            Rc::new(move || cell.get())
        }
    }

    pub fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    pub fn make_controller(managed: &[&str], restore: &str, grace: f64, clock: &FakeClock) -> StatusController {
        StatusController::new(restore, set(managed), grace, clock.clock())
    }

    fn plain() -> StatusController {
        make_controller(&[ONLINE], ONLINE, 0.0, &FakeClock::new())
    }

    fn s(v: &str) -> Option<String> {
        Some(v.to_string())
    }

    #[test]
    fn no_edge_returns_none() {
        assert_eq!(plain().evaluate(false, ONLINE), None);
    }

    #[test]
    fn no_edge_while_on_mobile() {
        let mut c = plain();
        assert_eq!(c.evaluate(true, ONLINE), s(IDLE));
        assert_eq!(c.evaluate(true, IDLE), None);
    }

    #[test]
    fn takes_control_from_online() {
        let mut c = plain();
        assert_eq!(c.evaluate(true, ONLINE), s(IDLE));
        assert!(c.holding);
        assert_eq!(c.saved, s(ONLINE));
    }

    #[test]
    fn hands_back_saved_status() {
        let mut c = plain();
        c.evaluate(true, ONLINE);
        assert_eq!(c.evaluate(false, IDLE), s(ONLINE));
        assert!(!c.holding);
        assert_eq!(c.saved, None);
    }

    #[test]
    fn repeated_polls_do_not_overwrite_saved() {
        let mut c = make_controller(&[ONLINE, DND], ONLINE, 0.0, &FakeClock::new());
        c.evaluate(true, DND);
        c.evaluate(true, IDLE);
        c.evaluate(true, IDLE);
        assert_eq!(c.saved, s(DND));
        assert_eq!(c.evaluate(false, IDLE), s(DND));
    }

    #[test]
    fn never_takes_control_of_hidden_status() {
        for status in [INVISIBLE, OFFLINE] {
            let mut c = plain();
            assert_eq!(c.evaluate(true, status), None);
            assert!(!c.holding);
            assert_eq!(c.evaluate(false, status), None);
        }
    }

    #[test]
    fn dnd_untouched_by_default() {
        let mut c = plain();
        assert_eq!(c.evaluate(true, DND), None);
        assert!(!c.holding);
    }

    #[test]
    fn dnd_managed_when_configured() {
        let mut c = make_controller(&[ONLINE, DND], ONLINE, 0.0, &FakeClock::new());
        assert_eq!(c.evaluate(true, DND), s(IDLE));
        assert_eq!(c.saved, s(DND));
        assert_eq!(c.evaluate(false, IDLE), s(DND));
    }

    #[test]
    fn adopts_existing_idle_after_restart() {
        let mut c = plain();
        assert_eq!(c.evaluate(true, IDLE), s(IDLE));
        assert!(c.holding);
        assert_eq!(c.saved, None);
        assert_eq!(c.evaluate(false, IDLE), s(ONLINE));
    }

    #[test]
    fn without_adopt_an_existing_hold_status_is_left_alone() {
        let mut c = plain();
        c.adopt = false;
        assert_eq!(c.evaluate(true, IDLE), None);
        assert!(!c.holding);
        assert_eq!(c.evaluate(false, IDLE), None);
        assert_eq!(c.evaluate(true, ONLINE), s(IDLE));
    }

    #[test]
    fn hand_back_without_control_is_noop() {
        let mut c = plain();
        c.evaluate(true, INVISIBLE);
        assert_eq!(c.evaluate(false, INVISIBLE), None);
    }

    #[test]
    fn manual_override_releases_control() {
        let mut c = plain();
        c.evaluate(true, ONLINE);
        assert_eq!(c.evaluate(true, ONLINE), None);
        assert!(!c.holding);
        assert_eq!(c.evaluate(false, ONLINE), None);
    }

    #[test]
    fn grace_window_survives_propagation_lag() {
        let clock = FakeClock::new();
        let mut c = make_controller(&[ONLINE], ONLINE, 25.0, &clock);
        assert_eq!(c.evaluate(true, ONLINE), s(IDLE));
        assert_eq!(c.evaluate(true, ONLINE), None);
        assert!(c.holding);
        clock.advance(24.0);
        assert_eq!(c.evaluate(true, ONLINE), None);
        assert!(c.holding);
        clock.advance(1.0);
        assert_eq!(c.evaluate(true, ONLINE), None);
        assert!(!c.holding);
    }

    #[test]
    fn hand_back_skipped_when_status_changed_under_us() {
        let mut c = make_controller(&[ONLINE], ONLINE, 100.0, &FakeClock::new());
        c.evaluate(true, ONLINE);
        assert_eq!(c.evaluate(false, INVISIBLE), None);
        assert!(!c.holding);
    }

    #[test]
    fn restore_falls_back_to_default_when_nothing_saved() {
        let mut c = plain();
        c.evaluate(true, IDLE);
        assert_eq!(c.saved, None);
        assert_eq!(c.evaluate(false, IDLE), s(ONLINE));
    }

    #[test]
    fn release_clears_state() {
        let mut c = plain();
        c.evaluate(true, ONLINE);
        c.release();
        assert!(!c.holding);
        assert_eq!(c.saved, None);
        assert_eq!(c.grace_until, 0.0);
    }

    #[test]
    fn controller_invariants_hold_for_random_walks() {
        let externals = [None, Some(ONLINE), Some(DND), Some(INVISIBLE), Some(OFFLINE)];
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..2000 {
            let clock = FakeClock::new();
            let mut c = make_controller(&[ONLINE], ONLINE, 2.0, &clock);
            let mut current = ONLINE.to_string();
            let steps = next() % 61;
            for _ in 0..steps {
                clock.advance(1.0);
                let on_mobile = next() % 2 == 0;
                if let Some(external) = externals[(next() % 5) as usize] {
                    current = external.to_string();
                }
                let target = c.evaluate(on_mobile, &current);
                assert_ne!(c.saved.as_deref(), Some(IDLE));
                match target {
                    Some(t) if t != current => {
                        assert!(current != INVISIBLE && current != OFFLINE);
                        assert!(t != INVISIBLE && t != OFFLINE);
                        current = t;
                    }
                    _ => {}
                }
            }
            for _ in 0..6 {
                if let Some(t) = c.evaluate(false, &current) {
                    current = t;
                }
            }
            assert!(!c.holding);
            assert_ne!(current, IDLE);
        }
    }

    #[test]
    fn debounce_requires_consecutive_readings() {
        let mut d = Debouncer::new(2, 3);
        assert!(!d.update(true));
        assert!(d.update(true));
    }

    #[test]
    fn debounce_flapping_never_flips() {
        let mut d = Debouncer::new(2, 2);
        for reading in [true, false, true, false, true, false] {
            assert!(!d.update(reading));
        }
    }

    #[test]
    fn debounce_off_threshold_is_independent() {
        let mut d = Debouncer::new(1, 3);
        assert!(d.update(true));
        assert!(d.update(false));
        assert!(d.update(false));
        assert!(!d.update(false));
    }

    #[test]
    fn debounce_reset_clears_pending_without_changing_stable() {
        let mut d = Debouncer::new(3, 3);
        d.update(true);
        d.update(true);
        d.reset();
        assert_eq!(d.count, 0);
        assert!(!d.stable);
        assert!(!d.update(true));
        assert!(!d.update(true));
        assert!(d.update(true));
    }

    #[test]
    fn debounce_initial_state_respected() {
        let mut d = Debouncer::with_initial(1, 1, true);
        assert!(d.update(true));
        assert!(!d.update(false));
    }

    #[test]
    fn debounce_steady_reading_resets_counter() {
        let mut d = Debouncer::new(2, 2);
        d.update(true);
        d.update(false);
        assert_eq!(d.count, 0);
        assert!(!d.pending);
    }
}
