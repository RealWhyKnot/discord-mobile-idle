use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::{sleep, sleep_until, timeout, Instant};

use crate::controller::{Clock, Debouncer, StatusController, INVISIBLE, OFFLINE};
use crate::persist::Marker;
use crate::{error, info, warning};

pub const LIVENESS_SECONDS: f64 = 1800.0;
pub const WRITE_TIMEOUT: f64 = 45.0;
pub const ECHO_SECONDS: f64 = 90.0;
pub const DISCONNECT_LIMIT: f64 = 900.0;
pub const CLOSE_WAIT: Duration = Duration::from_secs(15);
pub const UNKNOWN: &str = "unknown";

#[derive(Clone, Debug, PartialEq)]
pub struct Error {
    pub status: Option<u16>,
    pub message: String,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Error {
            status: None,
            message: message.into(),
        }
    }

    pub fn http(status: u16, message: impl Into<String>) -> Self {
        Error {
            status: Some(status),
            message: message.into(),
        }
    }

    pub fn rejected(&self) -> bool {
        self.status == Some(401)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.status {
            Some(status) => write!(f, "{} (HTTP {})", self.message, status),
            None => f.write_str(&self.message),
        }
    }
}

fn hidden(status: &str) -> bool {
    status == INVISIBLE || status == OFFLINE
}

fn py_bool(value: bool) -> &'static str {
    if value {
        "True"
    } else {
        "False"
    }
}

fn py_opt(value: &Option<String>) -> &str {
    value.as_deref().unwrap_or("None")
}

#[allow(async_fn_in_trait)]
pub trait Client {
    fn is_on_mobile(&self) -> bool;
    fn other_sessions(&self) -> usize;
    fn status(&self) -> String;
    fn saved_status(&self) -> String;
    fn own_status(&self) -> Option<String>;
    async fn check_token(&self) -> Result<(), Error>;
    async fn change_presence(&self, status: &str) -> Result<(), Error>;
    async fn hide(&self) -> Result<(), Error>;
    async fn unhide(&self, status: &str) -> Result<(), Error>;
}

#[derive(Debug)]
pub enum Note {
    Ready,
    Resumed,
    Disconnected,
    Sessions,
    Settings { after: String, before: String },
    Ended(Result<(), Error>),
}

pub trait Session {
    fn start(&mut self);
    fn close(&mut self);
    fn on_ready(&mut self, first: bool) -> bool;
}

pub enum Change<'a> {
    Wrote(&'a str),
    Shown(&'a str),
    Hidden { by_choice: bool },
}

pub type OnChange = Box<dyn Fn(Change)>;

pub struct Runner<C: Client, M: Marker> {
    pub client: C,
    pub controller: StatusController,
    pub debouncer: Debouncer,
    pub poll_seconds: u64,
    pub observe: bool,
    pub clock: Clock,
    pub marker: Option<M>,
    pub write_timeout: f64,
    pub echo_seconds: f64,
    pub stats: Option<Box<dyn Fn() -> String>>,
    pub on_change: Option<OnChange>,
    pub beat: Arc<AtomicU64>,
    pub pending: Option<String>,
    pub simulated: Option<String>,
    pub hidden: bool,
    pub returning: bool,
    pub stood_down: bool,
    pub following: bool,
    pub user_status: Option<String>,
    pub setting_echo: Option<String>,
    pub presence: Option<String>,
    pub expect: Option<(String, f64)>,
    pub reassert: bool,
    pub connected: bool,
    pub disconnected_at: Option<f64>,
    pub marked: Option<String>,
    pub gave_up: Option<String>,
    pub transitions: u64,
    pub wake: bool,
    pub started: f64,
    pub last_tick: f64,
    pub last_liveness: f64,
}

impl<C: Client, M: Marker> Runner<C, M> {
    pub fn new(
        client: C,
        controller: StatusController,
        debouncer: Debouncer,
        poll_seconds: u64,
        observe: bool,
        clock: Clock,
        marker: Option<M>,
    ) -> Self {
        let now = clock();
        Runner {
            client,
            controller,
            debouncer,
            poll_seconds,
            observe,
            clock,
            marker,
            write_timeout: WRITE_TIMEOUT,
            echo_seconds: ECHO_SECONDS,
            stats: None,
            on_change: None,
            beat: Arc::new(AtomicU64::new(now.to_bits())),
            pending: None,
            simulated: None,
            hidden: false,
            returning: false,
            stood_down: false,
            following: false,
            user_status: None,
            setting_echo: None,
            presence: None,
            expect: None,
            reassert: false,
            connected: true,
            disconnected_at: None,
            marked: None,
            gave_up: None,
            transitions: 0,
            wake: false,
            started: now,
            last_tick: now,
            last_liveness: now,
        }
    }

    fn now(&self) -> f64 {
        (self.clock)()
    }

    pub fn touch(&mut self) {
        self.last_tick = self.now();
        self.beat.store(self.last_tick.to_bits(), Ordering::Relaxed);
    }

    pub fn wake_if_mobile(&mut self) {
        if self.client.is_on_mobile() && !self.debouncer.stable {
            self.wake = true;
        }
    }

    pub fn recover(&mut self) {
        if self.observe {
            return;
        }
        let Some(saved) = self.marker.as_ref().and_then(|m| m.read()) else {
            return;
        };
        self.controller.resume_hold(&saved);
        self.debouncer.force(true);
        info!(
            "picking up a hold from before the restart, will restore {} once the phone goes",
            saved
        );
        self.marked = Some(saved);
    }

    pub fn seed_user_status(&mut self) {
        let status = self.client.saved_status();
        if status == UNKNOWN {
            return;
        }
        if status == self.controller.idle_status && self.idle_may_be_ours() {
            return;
        }
        self.user_status = Some(status);
    }

    fn idle_may_be_ours(&self) -> bool {
        if self.controller.holding {
            return true;
        }
        if self.marker.is_none() {
            return self.client.is_on_mobile();
        }
        false
    }

    pub fn note_settings(&mut self, status: &str, previous: Option<&str>) {
        if previous == Some(status) {
            return;
        }
        if self.setting_echo.as_deref() == Some(status) {
            self.setting_echo = None;
            return;
        }
        if status == self.controller.idle_status && self.controller.holding {
            return;
        }
        self.user_status = Some(status.to_string());
        self.wake = true;
    }

    pub fn note_connected(&mut self) {
        self.connected = true;
        self.disconnected_at = None;
        self.expect = None;
        self.reassert = self.presence.is_some();
        self.wake = true;
    }

    pub fn note_disconnected(&mut self) {
        if self.connected {
            self.connected = false;
            self.disconnected_at = Some(self.now());
        }
    }

    pub fn on_ready(&mut self, first: bool) {
        self.debouncer.reset();
        if first {
            self.recover();
        }
        self.seed_user_status();
        self.note_connected();
        self.touch();
    }

    pub fn apply(&mut self, note: Note) {
        match note {
            Note::Resumed => {
                info!("gateway resumed");
                self.debouncer.reset();
                self.note_connected();
                self.touch();
                self.wake_if_mobile();
            }
            Note::Disconnected => self.note_disconnected(),
            Note::Sessions => self.wake_if_mobile(),
            Note::Settings { after, before } => self.note_settings(&after, Some(&before)),
            Note::Ready | Note::Ended(_) => {}
        }
    }

    fn wants_hidden(&self) -> bool {
        self.user_status.as_deref().is_some_and(hidden)
    }

    fn wants_idle(&self) -> bool {
        self.user_status.as_deref() == Some(self.controller.idle_status)
    }

    pub async fn tick(&mut self) -> Result<(), Error> {
        let limit = Duration::from_secs_f64(self.write_timeout);
        let result = match timeout(limit, self.step()).await {
            Ok(result) => result,
            Err(_) => Err(Error::new(format!("timed out after {}s", self.write_timeout))),
        };
        self.sync_marker();
        result
    }

    async fn step(&mut self) -> Result<(), Error> {
        if !self.connected {
            let since = self.disconnected_at.unwrap_or_else(|| self.now());
            if self.now() - since > DISCONNECT_LIMIT {
                self.give_up(format!(
                    "no gateway connection for {} seconds, starting over",
                    DISCONNECT_LIMIT
                ));
            }
            return Ok(());
        }
        if self.reassert {
            let presence = self.presence.clone().unwrap_or_default();
            self.show(&presence).await?;
            self.reassert = false;
            info!("reconnected, sent {} again", presence);
        }
        if self.echo_missing() {
            let status = self.expect.as_ref().map(|e| e.0.clone()).unwrap_or_default();
            self.give_up(format!(
                "Discord never confirmed our {} presence, starting over",
                status
            ));
            return Ok(());
        }
        if self.wants_hidden() {
            return self.stand_down().await;
        }
        if self.stood_down {
            self.stand_up().await?;
        }
        let others = self.client.other_sessions();
        if self.hidden {
            if others > 0 {
                self.unhide().await?;
            }
            return Ok(());
        }
        if self.returning {
            if self.client.status() == INVISIBLE {
                return Ok(());
            }
            self.returning = false;
        }
        self.manage().await?;
        if others == 0 && !self.controller.holding && !self.debouncer.stable {
            self.hide().await?;
        }
        Ok(())
    }

    fn echo_missing(&mut self) -> bool {
        let Some((status, deadline)) = self.expect.clone() else {
            return false;
        };
        match self.client.own_status() {
            None => {
                self.expect = None;
                false
            }
            Some(own) if own == status || (hidden(&own) && hidden(&status)) => {
                self.expect = None;
                false
            }
            Some(_) => self.now() > deadline,
        }
    }

    pub fn give_up(&mut self, reason: String) {
        if self.gave_up.is_none() {
            error!("{}", reason);
            self.gave_up = Some(reason);
        }
    }

    fn chosen(&self) -> String {
        match self.user_status.as_deref() {
            None | Some(UNKNOWN) => self.controller.default_restore.clone(),
            Some(status) => status.to_string(),
        }
    }

    async fn write(&mut self, status: &str) -> Result<(), Error> {
        self.setting_echo = Some(status.to_string());
        self.client.change_presence(status).await?;
        self.sent(status);
        self.changed(Change::Wrote(status));
        Ok(())
    }

    async fn show(&mut self, status: &str) -> Result<(), Error> {
        if status == INVISIBLE {
            self.client.hide().await?;
            self.sent(status);
            self.changed(Change::Hidden {
                by_choice: self.wants_hidden(),
            });
        } else {
            self.client.unhide(status).await?;
            self.sent(status);
            self.changed(Change::Shown(status));
        }
        Ok(())
    }

    fn changed(&self, change: Change) {
        if let Some(on_change) = &self.on_change {
            on_change(change);
        }
    }

    fn sent(&mut self, status: &str) {
        self.presence = Some(status.to_string());
        self.expect = Some((status.to_string(), self.now() + self.echo_seconds));
    }

    async fn stand_down(&mut self) -> Result<(), Error> {
        if self.stood_down {
            return Ok(());
        }
        self.controller.stand_down();
        self.hidden = false;
        self.returning = false;
        self.following = false;
        if !self.observe {
            self.show(INVISIBLE).await?;
        }
        self.stood_down = true;
        info!("you set {}, leaving your status alone", py_opt(&self.user_status));
        Ok(())
    }

    async fn stand_up(&mut self) -> Result<(), Error> {
        self.debouncer.reset();
        let target = self.chosen();
        if !self.observe {
            self.show(&target).await?;
        }
        self.stood_down = false;
        self.returning = true;
        self.following = self.wants_idle();
        info!("you set {}, managing again", target);
        Ok(())
    }

    async fn hide(&mut self) -> Result<(), Error> {
        if !self.observe {
            self.show(INVISIBLE).await?;
        }
        self.hidden = true;
        info!("nobody connected, going offline");
        Ok(())
    }

    async fn unhide(&mut self) -> Result<(), Error> {
        let mut target = self.client.saved_status();
        if target == UNKNOWN || hidden(&target) {
            target = self.controller.default_restore.clone();
        }
        if !self.observe {
            self.show(&target).await?;
        }
        self.hidden = false;
        self.returning = true;
        info!("something connected, coming back as {}", target);
        Ok(())
    }

    async fn manage(&mut self) -> Result<(), Error> {
        let raw = self.client.is_on_mobile();
        let stable = self.debouncer.update(raw);
        if self.wants_idle() {
            return self.follow_idle(stable).await;
        }
        if self.following {
            self.stop_following().await?;
        }
        let current = self.simulated.clone().unwrap_or_else(|| self.client.status());
        if self.observe {
            info!(
                "observe: raw_mobile={} stable={} status={}",
                py_bool(raw),
                py_bool(stable),
                current
            );
        }
        if self.pending.is_none() {
            self.pending = self.controller.evaluate(stable, &current);
        }
        let Some(pending) = self.pending.clone() else {
            return Ok(());
        };
        if pending == current {
            self.pending = None;
            return Ok(());
        }
        if self.observe {
            info!(
                "observe: would set {} -> {} (on_mobile={})",
                current,
                pending,
                py_bool(stable)
            );
            self.simulated = Some(pending);
            self.pending = None;
            return Ok(());
        }
        self.write(&pending).await?;
        self.transitions += 1;
        info!("status {} -> {} (on_mobile={})", current, pending, py_bool(stable));
        self.pending = None;
        Ok(())
    }

    async fn follow_idle(&mut self, stable: bool) -> Result<(), Error> {
        self.controller.step_aside(stable);
        self.pending = None;
        if self.following {
            return Ok(());
        }
        if !self.observe {
            let idle = self.controller.idle_status;
            self.show(idle).await?;
        }
        self.following = true;
        info!("you set idle, leaving it alone until you pick another status");
        Ok(())
    }

    async fn stop_following(&mut self) -> Result<(), Error> {
        let target = self.chosen();
        if !self.observe {
            self.show(&target).await?;
        }
        self.following = false;
        info!("you set {}, managing again", target);
        Ok(())
    }

    fn sync_marker(&mut self) {
        if self.observe {
            return;
        }
        let Some(marker) = self.marker.as_mut() else {
            return;
        };
        let want = if self.controller.holding {
            Some(self.controller.restore_target())
        } else {
            None
        };
        if want == self.marked {
            return;
        }
        match &want {
            None => marker.clear(),
            Some(status) => marker.write(status),
        }
        self.marked = want;
    }

    pub async fn hand_back(&mut self) -> Result<(), Error> {
        if self.observe || !self.controller.holding {
            return Ok(());
        }
        let target = self.controller.restore_target();
        let current = self.client.status();
        if current == self.controller.idle_status || current == target {
            self.write(&target).await?;
            info!("restored {} on shutdown", target);
        }
        self.controller.release();
        self.sync_marker();
        Ok(())
    }

    pub async fn cycle(&mut self) {
        if let Err(e) = self.tick().await {
            if e.rejected() {
                self.give_up("Discord rejected the token, it needs replacing".into());
            } else {
                error!("tick failed: {}", e);
            }
        }
        self.touch();
        self.liveness().await;
    }

    pub async fn liveness(&mut self) {
        let now = self.now();
        if now - self.last_liveness < LIVENESS_SECONDS {
            return;
        }
        self.last_liveness = now;
        info!(
            "alive uptime={:.0}s transitions={} on_mobile={} holding={} hidden={} returning={} following={} you={}",
            now - self.started,
            self.transitions,
            py_bool(self.debouncer.stable),
            py_bool(self.controller.holding),
            py_bool(self.hidden),
            py_bool(self.returning),
            py_bool(self.following),
            py_opt(&self.user_status)
        );
        if let Some(stats) = &self.stats {
            info!("ignored gateway events: {}", stats());
        }
        if self.observe || !self.connected {
            return;
        }
        let limit = Duration::from_secs_f64(self.write_timeout);
        match timeout(limit, self.client.check_token()).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) if e.rejected() => self.give_up("Discord rejected the token, it needs replacing".into()),
            Ok(Err(e)) => warning!("token check failed: {}", e),
            Err(_) => warning!("token check failed: timed out"),
        }
    }
}

enum Outcome {
    Ended(Result<(), Error>),
    Stopped,
    GaveUp,
    Closing,
}

async fn wait_for_end(notes: &mut UnboundedReceiver<Note>) -> Result<(), Error> {
    let deadline = Instant::now() + CLOSE_WAIT;
    loop {
        match tokio::time::timeout_at(deadline, notes.recv()).await {
            Ok(Some(Note::Ended(result))) => return result,
            Ok(Some(_)) => continue,
            Ok(None) => return Ok(()),
            Err(_) => {
                warning!("the gateway did not close within {}s", CLOSE_WAIT.as_secs());
                return Ok(());
            }
        }
    }
}

pub async fn serve<C: Client, M: Marker, S: Session>(
    runner: &mut Runner<C, M>,
    session: &mut S,
    notes: &mut UnboundedReceiver<Note>,
    mut stop: Pin<&mut dyn Future<Output = ()>>,
    delay: u64,
) -> Result<i32, Error> {
    if delay > 0 {
        warning!(
            "restarted too often in the last hour, waiting {}s before logging in",
            delay
        );
        tokio::select! {
            _ = &mut stop => return Ok(0),
            _ = sleep(Duration::from_secs(delay)) => {}
        }
    }
    session.start();
    let mut ticking = false;
    let mut due = false;
    let mut deadline = Instant::now();
    let outcome = loop {
        if ticking && (due || runner.wake) {
            runner.wake = false;
            due = false;
            let stopped = tokio::select! {
                biased;
                _ = &mut stop => true,
                _ = runner.cycle() => false,
            };
            if stopped {
                runner.sync_marker();
                break Outcome::Stopped;
            }
            if runner.gave_up.is_some() {
                break Outcome::GaveUp;
            }
            deadline = Instant::now() + Duration::from_secs(runner.poll_seconds);
            continue;
        }
        tokio::select! {
            _ = &mut stop => break Outcome::Stopped,
            note = notes.recv() => match note {
                None => break Outcome::Ended(Ok(())),
                Some(Note::Ended(result)) => break Outcome::Ended(result),
                Some(Note::Ready) => {
                    let first = !ticking;
                    let close = session.on_ready(first);
                    runner.on_ready(first);
                    if close {
                        session.close();
                        break Outcome::Closing;
                    }
                    if first {
                        ticking = true;
                        due = true;
                    }
                }
                Some(other) => runner.apply(other),
            },
            _ = sleep_until(deadline), if ticking => due = true,
        }
    };
    match outcome {
        Outcome::Ended(result) => result?,
        Outcome::Stopped if runner.gave_up.is_none() => {
            info!("stop requested");
            let limit = Duration::from_secs_f64(runner.write_timeout);
            match timeout(limit, runner.hand_back()).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => error!("could not restore the status on the way out: {}", e),
                Err(_) => error!("could not restore the status on the way out: timed out"),
            }
            session.close();
            wait_for_end(notes).await?;
        }
        Outcome::Stopped | Outcome::GaveUp => {
            session.close();
            wait_for_end(notes).await?;
        }
        Outcome::Closing => wait_for_end(notes).await?,
    }
    Ok(if runner.gave_up.is_none() { 0 } else { 1 })
}

pub struct Watchdog {
    pub last_tick: Box<dyn Fn() -> f64 + Send>,
    pub timeout: f64,
    pub action: &'static str,
    pub on_stall: Box<dyn FnMut() + Send>,
    pub clock: Box<dyn Fn() -> f64 + Send>,
    pub fired: bool,
}

impl Watchdog {
    pub fn check(&mut self) -> bool {
        if self.fired {
            return false;
        }
        let stalled = (self.clock)() - (self.last_tick)();
        if stalled <= self.timeout {
            return false;
        }
        self.fired = true;
        error!("watchdog: no tick for {:.0}s, {}", stalled, self.action);
        (self.on_stall)();
        true
    }

    pub fn start(mut self, interval: f64) {
        std::thread::Builder::new()
            .name("watchdog".into())
            .stack_size(64 * 1024)
            .spawn(move || loop {
                std::thread::sleep(Duration::from_secs_f64(interval));
                if self.check() {
                    return;
                }
            })
            .expect("watchdog thread");
    }
}

pub fn beat_reader(beat: Arc<AtomicU64>) -> impl Fn() -> f64 + Send {
    move || f64::from_bits(beat.load(Ordering::Relaxed))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::controller::tests::{make_controller, FakeClock};
    use crate::controller::{DND, IDLE, ONLINE};
    use crate::log;
    use std::cell::RefCell;
    use std::rc::Rc;
    use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
    use tokio::sync::oneshot;

    pub type WriteHook = Box<dyn Fn(&str) -> (String, String)>;

    pub struct Fake {
        pub status: String,
        pub on_mobile: bool,
        pub others: usize,
        pub saved: String,
        pub echo: bool,
        pub calls: Vec<String>,
        pub presence: Vec<String>,
        pub raises: Option<Error>,
        pub during_write: Option<WriteHook>,
        pub echoes: Vec<(String, String)>,
        pub own: Option<String>,
        pub token_error: Option<Error>,
        pub token_checks: u32,
        pub hang: bool,
        pub unhide_error: Option<Error>,
    }

    #[derive(Clone)]
    pub struct FakeClient(pub Rc<RefCell<Fake>>);

    impl FakeClient {
        pub fn new() -> Self {
            FakeClient::with(ONLINE, false, 1, None)
        }

        pub fn with(status: &str, on_mobile: bool, others: usize, saved: Option<&str>) -> Self {
            FakeClient(Rc::new(RefCell::new(Fake {
                status: status.into(),
                on_mobile,
                others,
                saved: saved.unwrap_or(status).into(),
                echo: true,
                calls: vec![],
                presence: vec![],
                raises: None,
                during_write: None,
                echoes: vec![],
                own: None,
                token_error: None,
                token_checks: 0,
                hang: false,
                unhide_error: None,
            })))
        }

        pub fn f(&self) -> std::cell::RefMut<'_, Fake> {
            self.0.borrow_mut()
        }

        pub fn set_mobile(&self, value: bool) {
            self.f().on_mobile = value;
        }

        pub fn set_status(&self, value: &str) {
            self.f().status = value.into();
        }

        pub fn calls(&self) -> Vec<String> {
            self.0.borrow().calls.clone()
        }

        pub fn presence(&self) -> Vec<String> {
            self.0.borrow().presence.clone()
        }

        pub fn settle(&self) {
            let mut f = self.f();
            f.status = f.presence.last().cloned().unwrap();
        }
    }

    impl Client for FakeClient {
        fn is_on_mobile(&self) -> bool {
            self.0.borrow().on_mobile
        }
        fn other_sessions(&self) -> usize {
            self.0.borrow().others
        }
        fn status(&self) -> String {
            self.0.borrow().status.clone()
        }
        fn saved_status(&self) -> String {
            self.0.borrow().saved.clone()
        }
        fn own_status(&self) -> Option<String> {
            self.0.borrow().own.clone()
        }
        async fn check_token(&self) -> Result<(), Error> {
            let mut f = self.f();
            f.token_checks += 1;
            match &f.token_error {
                Some(e) => Err(e.clone()),
                None => Ok(()),
            }
        }
        async fn change_presence(&self, status: &str) -> Result<(), Error> {
            let hang = {
                let mut f = self.f();
                f.calls.push(status.into());
                if let Some(hook) = &f.during_write {
                    let echo = hook(status);
                    f.echoes.push(echo);
                }
                f.hang
            };
            if hang {
                std::future::pending::<()>().await;
            }
            let mut f = self.f();
            if let Some(e) = &f.raises {
                return Err(e.clone());
            }
            f.status = status.into();
            Ok(())
        }
        async fn hide(&self) -> Result<(), Error> {
            let mut f = self.f();
            f.presence.push(INVISIBLE.into());
            f.status = INVISIBLE.into();
            Ok(())
        }
        async fn unhide(&self, status: &str) -> Result<(), Error> {
            let mut f = self.f();
            if let Some(e) = &f.unhide_error {
                return Err(e.clone());
            }
            f.presence.push(status.into());
            if f.echo {
                f.status = status.into();
            }
            Ok(())
        }
    }

    pub type MarkerLog = Rc<RefCell<(Option<String>, Vec<Option<String>>)>>;

    #[derive(Clone, Default)]
    pub struct MemoryMarker(pub MarkerLog);

    impl MemoryMarker {
        pub fn holding(value: &str) -> Self {
            MemoryMarker(Rc::new(RefCell::new((Some(value.into()), vec![]))))
        }
        pub fn value(&self) -> Option<String> {
            self.0.borrow().0.clone()
        }
        pub fn writes(&self) -> Vec<Option<String>> {
            self.0.borrow().1.clone()
        }
    }

    impl Marker for MemoryMarker {
        fn read(&self) -> Option<String> {
            self.value()
        }
        fn write(&mut self, status: &str) {
            let mut m = self.0.borrow_mut();
            m.0 = Some(status.into());
            m.1.push(Some(status.into()));
        }
        fn clear(&mut self) {
            let mut m = self.0.borrow_mut();
            m.0 = None;
            m.1.push(None);
        }
    }

    pub struct Opts {
        pub managed: Vec<&'static str>,
        pub restore: &'static str,
        pub on_polls: u32,
        pub off_polls: u32,
        pub observe: bool,
        pub clock: FakeClock,
        pub grace: f64,
        pub marker: Option<MemoryMarker>,
        pub echo_seconds: f64,
        pub write_timeout: f64,
    }

    impl Default for Opts {
        fn default() -> Self {
            Opts {
                managed: vec![ONLINE],
                restore: ONLINE,
                on_polls: 1,
                off_polls: 1,
                observe: false,
                clock: FakeClock::new(),
                grace: 0.0,
                marker: None,
                echo_seconds: ECHO_SECONDS,
                write_timeout: WRITE_TIMEOUT,
            }
        }
    }

    pub type TestRunner = Runner<FakeClient, MemoryMarker>;

    pub fn make(client: &FakeClient, o: Opts) -> TestRunner {
        let controller = make_controller(&o.managed, o.restore, o.grace, &o.clock);
        let mut runner = Runner::new(
            client.clone(),
            controller,
            Debouncer::new(o.on_polls, o.off_polls),
            0,
            o.observe,
            o.clock.clock(),
            o.marker,
        );
        runner.echo_seconds = o.echo_seconds;
        runner.write_timeout = o.write_timeout;
        runner
    }

    pub fn plain(client: &FakeClient) -> TestRunner {
        make(client, Opts::default())
    }

    pub async fn drive(runner: &mut TestRunner, ticks: usize) {
        for _ in 0..ticks {
            runner.tick().await.unwrap();
        }
    }

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn s(value: &str) -> Option<String> {
        Some(value.to_string())
    }

    fn rejected() -> Error {
        Error::http(401, "Unauthorized")
    }

    fn server_error() -> Error {
        Error::http(500, "Server Error")
    }

    fn apply_echoes(runner: &mut TestRunner) {
        let echoes = std::mem::take(&mut runner.client.f().echoes);
        for (after, before) in echoes {
            runner.note_settings(&after, Some(&before));
        }
    }

    #[tokio::test]
    async fn full_lifecycle_writes_once_each_way() {
        let client = FakeClient::new();
        let mut runner = plain(&client);
        runner.tick().await.unwrap();
        assert!(client.calls().is_empty());

        client.set_mobile(true);
        runner.tick().await.unwrap();
        assert_eq!(client.calls(), v(&[IDLE]));
        assert_eq!(client.status(), IDLE);

        runner.tick().await.unwrap();
        assert_eq!(client.calls().len(), 1);

        client.set_mobile(false);
        runner.tick().await.unwrap();
        assert_eq!(client.calls(), v(&[IDLE, ONLINE]));
        assert_eq!(client.status(), ONLINE);
    }

    #[tokio::test]
    async fn saved_status_is_restored_not_default() {
        let client = FakeClient::with(DND, false, 1, None);
        let mut runner = make(
            &client,
            Opts {
                managed: vec![ONLINE, DND],
                ..Opts::default()
            },
        );
        client.set_mobile(true);
        runner.tick().await.unwrap();
        assert_eq!(client.status(), IDLE);
        client.set_mobile(false);
        runner.tick().await.unwrap();
        assert_eq!(client.status(), DND);
    }

    #[tokio::test]
    async fn adopted_idle_is_not_rewritten() {
        let client = FakeClient::with(IDLE, true, 1, None);
        let mut runner = plain(&client);
        runner.tick().await.unwrap();
        assert!(client.calls().is_empty());
        assert!(runner.controller.holding);
        client.set_mobile(false);
        runner.tick().await.unwrap();
        assert_eq!(client.calls(), v(&[ONLINE]));
    }

    #[tokio::test]
    async fn hidden_status_is_never_touched() {
        let client = FakeClient::with(INVISIBLE, false, 1, None);
        let mut runner = plain(&client);
        client.set_mobile(true);
        runner.tick().await.unwrap();
        client.set_mobile(false);
        runner.tick().await.unwrap();
        assert!(client.calls().is_empty());
        assert_eq!(client.status(), INVISIBLE);
    }

    #[tokio::test]
    async fn observe_mode_writes_nothing() {
        let client = FakeClient::new();
        let mut runner = make(
            &client,
            Opts {
                observe: true,
                ..Opts::default()
            },
        );
        client.set_mobile(true);
        runner.tick().await.unwrap();
        assert!(client.calls().is_empty());
        assert_eq!(client.status(), ONLINE);
    }

    #[tokio::test]
    async fn observe_mode_simulates_its_own_writes() {
        let client = FakeClient::new();
        let mut runner = make(
            &client,
            Opts {
                observe: true,
                ..Opts::default()
            },
        );
        client.set_mobile(true);
        runner.tick().await.unwrap();
        assert_eq!(runner.simulated, s(IDLE));
        client.set_mobile(false);
        runner.tick().await.unwrap();
        assert_eq!(runner.simulated, s(ONLINE));
        assert!(client.calls().is_empty());
        assert_eq!(client.status(), ONLINE);
    }

    #[tokio::test]
    async fn failed_write_is_retried_on_a_later_tick() {
        let client = FakeClient::new();
        let mut runner = plain(&client);
        client.set_mobile(true);
        client.f().raises = Some(Error::new("boom"));
        assert!(runner.tick().await.is_err());
        assert_eq!(runner.pending, s(IDLE));
        client.f().raises = None;
        runner.tick().await.unwrap();
        assert_eq!(client.calls(), v(&[IDLE, IDLE]));
        assert_eq!(client.status(), IDLE);
        assert_eq!(runner.pending, None);
    }

    #[tokio::test]
    async fn cycles_survive_errors_and_keep_ticking() {
        let o = Opts::default();
        let clock = o.clock.clone();
        let client = FakeClient::new();
        let mut runner = make(&client, o);
        client.set_mobile(true);
        client.f().raises = Some(Error::new("boom"));
        let start = runner.last_tick;
        for _ in 0..3 {
            runner.cycle().await;
            clock.advance(10.0);
        }
        assert_eq!(client.calls().len(), 3);
        assert!(runner.last_tick > start);
        assert!(runner.gave_up.is_none());
    }

    #[tokio::test]
    async fn cycles_record_transitions() {
        let o = Opts::default();
        let clock = o.clock.clone();
        let client = FakeClient::new();
        let mut runner = make(&client, o);
        client.set_mobile(true);
        for _ in 0..2 {
            runner.cycle().await;
            clock.advance(10.0);
        }
        assert_eq!(runner.transitions, 1);
        assert_eq!(client.status(), IDLE);
    }

    #[tokio::test]
    async fn liveness_logs_after_the_interval() {
        let o = Opts::default();
        let clock = o.clock.clone();
        let client = FakeClient::new();
        let mut runner = make(&client, o);
        log::take();
        for _ in 0..3 {
            runner.cycle().await;
            clock.advance(1000.0);
        }
        let alive = log::take().iter().filter(|(_, m)| m.contains("alive uptime=")).count();
        assert_eq!(alive, 1);
    }

    #[tokio::test]
    async fn hand_back_writes_nothing_when_not_holding() {
        let client = FakeClient::new();
        let mut runner = plain(&client);
        runner.hand_back().await.unwrap();
        assert!(client.calls().is_empty());
    }

    #[tokio::test]
    async fn hand_back_writes_nothing_in_observe_mode() {
        let client = FakeClient::new();
        let mut runner = make(
            &client,
            Opts {
                observe: true,
                ..Opts::default()
            },
        );
        client.set_mobile(true);
        runner.tick().await.unwrap();
        runner.hand_back().await.unwrap();
        assert!(client.calls().is_empty());
        assert!(runner.controller.holding);
    }

    #[tokio::test]
    async fn hand_back_restores_the_saved_status() {
        let client = FakeClient::with(DND, false, 1, None);
        let mut runner = make(
            &client,
            Opts {
                managed: vec![ONLINE, DND],
                ..Opts::default()
            },
        );
        client.set_mobile(true);
        runner.tick().await.unwrap();
        assert_eq!(client.status(), IDLE);
        runner.hand_back().await.unwrap();
        assert_eq!(client.status(), DND);
    }

    #[tokio::test]
    async fn hand_back_falls_back_to_the_default_restore() {
        let client = FakeClient::with(IDLE, false, 1, None);
        let mut runner = plain(&client);
        client.set_mobile(true);
        runner.tick().await.unwrap();
        runner.hand_back().await.unwrap();
        assert_eq!(client.status(), ONLINE);
    }

    #[tokio::test]
    async fn wake_if_mobile_sets_on_rising_edge() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        let mut runner = plain(&client);
        runner.wake_if_mobile();
        assert!(runner.wake);
    }

    #[tokio::test]
    async fn wake_if_mobile_ignores_when_no_mobile() {
        let client = FakeClient::new();
        let mut runner = plain(&client);
        runner.wake_if_mobile();
        assert!(!runner.wake);
    }

    #[tokio::test]
    async fn wake_if_mobile_ignores_when_already_stable() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        let mut runner = plain(&client);
        runner.debouncer.stable = true;
        runner.wake_if_mobile();
        assert!(!runner.wake);
    }

    #[tokio::test]
    async fn hand_back_leaves_a_manual_override_alone() {
        let client = FakeClient::new();
        let mut runner = plain(&client);
        client.set_mobile(true);
        runner.tick().await.unwrap();
        client.set_status(DND);
        client.f().calls.clear();
        runner.hand_back().await.unwrap();
        assert!(client.calls().is_empty());
    }

    #[tokio::test]
    async fn hand_back_restores_when_the_idle_write_has_not_echoed() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        let mut runner = plain(&client);
        runner.tick().await.unwrap();
        assert_eq!(client.calls(), v(&[IDLE]));
        client.set_status(ONLINE);
        runner.hand_back().await.unwrap();
        assert_eq!(client.calls(), v(&[IDLE, ONLINE]));
    }

    #[tokio::test]
    async fn hand_back_still_refuses_a_status_someone_else_set() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        let mut runner = plain(&client);
        runner.tick().await.unwrap();
        client.set_status(INVISIBLE);
        runner.hand_back().await.unwrap();
        assert_eq!(client.calls(), v(&[IDLE]));
    }

    #[tokio::test]
    async fn choosing_idle_pulls_our_session_to_idle_without_touching_settings() {
        let client = FakeClient::new();
        let mut runner = plain(&client);
        runner.tick().await.unwrap();
        runner.note_settings(IDLE, Some(ONLINE));
        runner.tick().await.unwrap();
        assert!(client.calls().is_empty());
        assert_eq!(client.presence(), v(&[IDLE]));
        assert!(runner.following);
    }

    #[tokio::test]
    async fn a_phone_cycle_leaves_your_idle_alone() {
        let client = FakeClient::with(IDLE, false, 1, None);
        let mut runner = plain(&client);
        runner.note_settings(IDLE, Some(ONLINE));
        runner.tick().await.unwrap();
        client.set_mobile(true);
        drive(&mut runner, 3).await;
        client.set_mobile(false);
        drive(&mut runner, 3).await;
        assert!(client.calls().is_empty());
        assert_eq!(client.presence(), v(&[IDLE]));
        assert!(!runner.controller.holding);
    }

    #[tokio::test]
    async fn overriding_the_phone_then_choosing_idle_is_never_restored() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        let mut runner = plain(&client);
        runner.tick().await.unwrap();
        runner.note_settings(IDLE, Some(ONLINE));
        runner.note_settings(DND, Some(IDLE));
        client.set_status(DND);
        runner.tick().await.unwrap();
        runner.note_settings(IDLE, Some(DND));
        client.set_status(IDLE);
        runner.tick().await.unwrap();
        client.set_mobile(false);
        drive(&mut runner, 3).await;
        assert_eq!(client.calls(), v(&[IDLE]));
        assert_eq!(client.presence(), v(&[IDLE]));
        assert!(!runner.controller.holding);
    }

    #[tokio::test]
    async fn picking_online_again_brings_our_session_back_and_waits_for_the_next_pickup() {
        let client = FakeClient::with(IDLE, true, 1, None);
        let mut runner = plain(&client);
        runner.note_settings(IDLE, Some(ONLINE));
        runner.tick().await.unwrap();
        runner.note_settings(ONLINE, Some(IDLE));
        client.set_status(ONLINE);
        drive(&mut runner, 2).await;
        assert_eq!(client.presence(), v(&[IDLE, ONLINE]));
        assert!(client.calls().is_empty());
        assert!(!runner.following);
        client.set_mobile(false);
        drive(&mut runner, 2).await;
        client.set_mobile(true);
        runner.tick().await.unwrap();
        assert_eq!(client.calls(), v(&[IDLE]));
    }

    #[tokio::test]
    async fn an_unset_status_after_idle_falls_back_to_the_restore_status() {
        let client = FakeClient::with(IDLE, false, 1, None);
        let mut runner = plain(&client);
        runner.note_settings(IDLE, Some(ONLINE));
        runner.tick().await.unwrap();
        runner.note_settings(UNKNOWN, Some(IDLE));
        runner.tick().await.unwrap();
        assert_eq!(client.presence(), v(&[IDLE, ONLINE]));
    }

    #[tokio::test]
    async fn alone_while_idle_still_goes_offline_and_comes_back_idle() {
        let client = FakeClient::with(IDLE, false, 1, Some(IDLE));
        let mut runner = plain(&client);
        runner.note_settings(IDLE, Some(ONLINE));
        runner.tick().await.unwrap();
        client.f().others = 0;
        runner.tick().await.unwrap();
        client.f().others = 1;
        drive(&mut runner, 2).await;
        assert_eq!(client.presence(), v(&[IDLE, INVISIBLE, IDLE]));
        assert!(runner.following);
        assert!(client.calls().is_empty());
    }

    #[tokio::test]
    async fn invisible_then_idle_comes_back_idle_once() {
        let client = FakeClient::new();
        let mut runner = plain(&client);
        runner.note_settings(INVISIBLE, Some(ONLINE));
        runner.tick().await.unwrap();
        runner.note_settings(IDLE, Some(INVISIBLE));
        drive(&mut runner, 3).await;
        assert_eq!(client.presence(), v(&[INVISIBLE, IDLE]));
        assert!(runner.following);
    }

    #[tokio::test]
    async fn idle_then_invisible_stops_following() {
        let client = FakeClient::new();
        let mut runner = plain(&client);
        runner.note_settings(IDLE, Some(ONLINE));
        runner.tick().await.unwrap();
        runner.note_settings(INVISIBLE, Some(IDLE));
        runner.tick().await.unwrap();
        assert!(!runner.following);
        assert_eq!(client.presence(), v(&[IDLE, INVISIBLE]));
    }

    #[tokio::test]
    async fn an_unchanged_status_in_a_settings_update_is_ignored() {
        let client = FakeClient::with(IDLE, false, 1, Some(IDLE));
        let mut runner = plain(&client);
        runner.note_settings(IDLE, Some(IDLE));
        assert_eq!(runner.user_status, None);
        assert!(!runner.wake);
    }

    #[tokio::test]
    async fn a_saved_idle_at_login_with_no_phone_is_yours() {
        let client = FakeClient::with(IDLE, false, 1, Some(IDLE));
        let mut runner = plain(&client);
        runner.seed_user_status();
        client.set_mobile(true);
        drive(&mut runner, 2).await;
        client.set_mobile(false);
        drive(&mut runner, 2).await;
        assert_eq!(runner.user_status, s(IDLE));
        assert!(client.calls().is_empty());
    }

    #[tokio::test]
    async fn a_reconnect_while_we_hold_idle_does_not_read_it_as_yours() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        let mut runner = plain(&client);
        runner.tick().await.unwrap();
        client.f().saved = IDLE.into();
        client.set_mobile(false);
        runner.seed_user_status();
        runner.tick().await.unwrap();
        assert_eq!(runner.user_status, None);
        assert_eq!(client.calls(), v(&[IDLE, ONLINE]));
    }

    #[tokio::test]
    async fn with_a_marker_and_no_hold_a_saved_idle_is_yours_even_with_the_phone_on() {
        let client = FakeClient::with(IDLE, true, 1, Some(IDLE));
        let mut runner = make(
            &client,
            Opts {
                marker: Some(MemoryMarker::default()),
                ..Opts::default()
            },
        );
        runner.recover();
        runner.seed_user_status();
        runner.tick().await.unwrap();
        client.set_mobile(false);
        drive(&mut runner, 2).await;
        assert_eq!(runner.user_status, s(IDLE));
        assert!(client.calls().is_empty());
    }

    #[tokio::test]
    async fn with_a_marker_from_a_hold_a_saved_idle_is_ours() {
        let client = FakeClient::with(IDLE, true, 1, Some(IDLE));
        let mut runner = make(
            &client,
            Opts {
                marker: Some(MemoryMarker::holding(ONLINE)),
                ..Opts::default()
            },
        );
        runner.recover();
        runner.seed_user_status();
        runner.tick().await.unwrap();
        client.set_mobile(false);
        runner.tick().await.unwrap();
        assert_eq!(runner.user_status, None);
        assert_eq!(client.calls(), v(&[ONLINE]));
    }

    #[tokio::test]
    async fn a_saved_idle_at_login_with_the_phone_on_is_not_trusted() {
        let client = FakeClient::with(IDLE, true, 1, Some(IDLE));
        let mut runner = plain(&client);
        runner.seed_user_status();
        runner.tick().await.unwrap();
        client.set_mobile(false);
        runner.tick().await.unwrap();
        assert_eq!(runner.user_status, None);
        assert_eq!(client.calls(), v(&[ONLINE]));
    }

    #[tokio::test]
    async fn a_reconnect_keeps_the_idle_you_chose() {
        let client = FakeClient::with(IDLE, false, 1, Some(IDLE));
        let mut runner = plain(&client);
        runner.note_settings(IDLE, Some(ONLINE));
        runner.seed_user_status();
        assert_eq!(runner.user_status, s(IDLE));
    }

    #[tokio::test]
    async fn echoes_of_our_own_writes_are_not_read_as_your_choice() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        let mut runner = plain(&client);
        client.f().during_write = Some(Box::new(|status| {
            (
                status.into(),
                if status == ONLINE { IDLE.into() } else { ONLINE.into() },
            )
        }));
        runner.tick().await.unwrap();
        apply_echoes(&mut runner);
        client.set_mobile(false);
        runner.tick().await.unwrap();
        apply_echoes(&mut runner);
        assert_eq!(client.calls(), v(&[IDLE, ONLINE]));
        assert_eq!(runner.user_status, None);
        assert_eq!(runner.setting_echo, None);
    }

    #[tokio::test]
    async fn an_echo_of_the_shutdown_restore_is_still_ours() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        let mut runner = plain(&client);
        runner.tick().await.unwrap();
        client.f().during_write = Some(Box::new(|status| (status.into(), IDLE.into())));
        runner.hand_back().await.unwrap();
        apply_echoes(&mut runner);
        assert_eq!(runner.user_status, None);
        assert_eq!(runner.setting_echo, None);
    }

    #[tokio::test]
    async fn observe_mode_follows_idle_without_writing() {
        let client = FakeClient::new();
        let mut runner = make(
            &client,
            Opts {
                observe: true,
                ..Opts::default()
            },
        );
        runner.note_settings(IDLE, Some(ONLINE));
        runner.tick().await.unwrap();
        assert!(runner.following);
        runner.note_settings(ONLINE, Some(IDLE));
        runner.tick().await.unwrap();
        assert!(client.presence().is_empty());
        assert!(!runner.following);
    }

    #[tokio::test]
    async fn goes_offline_when_nothing_else_is_connected() {
        let client = FakeClient::with(ONLINE, false, 0, None);
        let mut runner = plain(&client);
        runner.tick().await.unwrap();
        assert_eq!(client.presence(), v(&[INVISIBLE]));
        assert!(client.calls().is_empty());
        assert!(runner.hidden);
    }

    #[tokio::test]
    async fn staying_alone_is_not_written_again() {
        let client = FakeClient::with(ONLINE, false, 0, None);
        let mut runner = plain(&client);
        drive(&mut runner, 4).await;
        assert_eq!(client.presence(), v(&[INVISIBLE]));
    }

    #[tokio::test]
    async fn comes_back_when_something_connects() {
        let client = FakeClient::with(ONLINE, false, 0, None);
        let mut runner = plain(&client);
        runner.tick().await.unwrap();
        client.f().others = 1;
        runner.tick().await.unwrap();
        assert_eq!(client.presence(), v(&[INVISIBLE, ONLINE]));
        assert!(client.calls().is_empty());
        assert!(!runner.hidden);
    }

    #[tokio::test]
    async fn comes_back_as_the_saved_status() {
        let client = FakeClient::with(ONLINE, false, 0, Some(DND));
        let mut runner = plain(&client);
        runner.tick().await.unwrap();
        client.f().others = 1;
        runner.tick().await.unwrap();
        assert_eq!(client.presence(), v(&[INVISIBLE, DND]));
    }

    #[tokio::test]
    async fn an_unset_saved_status_falls_back_to_the_configured_restore() {
        let client = FakeClient::with(ONLINE, false, 0, Some(UNKNOWN));
        let mut runner = make(
            &client,
            Opts {
                restore: DND,
                ..Opts::default()
            },
        );
        runner.tick().await.unwrap();
        client.f().others = 1;
        runner.tick().await.unwrap();
        assert_eq!(client.presence(), v(&[INVISIBLE, DND]));
    }

    #[tokio::test]
    async fn the_phone_is_handed_back_before_going_offline() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        let mut runner = plain(&client);
        runner.tick().await.unwrap();
        assert_eq!(client.calls(), v(&[IDLE]));
        client.set_mobile(false);
        client.f().others = 0;
        runner.tick().await.unwrap();
        assert_eq!(client.calls(), v(&[IDLE, ONLINE]));
        assert_eq!(client.presence(), v(&[INVISIBLE]));
    }

    #[tokio::test]
    async fn never_goes_offline_while_holding_the_status() {
        let client = FakeClient::with(ONLINE, true, 0, None);
        let mut runner = plain(&client);
        runner.tick().await.unwrap();
        assert_eq!(client.calls(), v(&[IDLE]));
        assert!(client.presence().is_empty());
        assert!(!runner.hidden);
    }

    #[tokio::test]
    async fn never_goes_offline_while_the_phone_still_reads_connected() {
        let client = FakeClient::with(DND, true, 0, None);
        let mut runner = plain(&client);
        runner.tick().await.unwrap();
        assert!(client.calls().is_empty());
        assert!(client.presence().is_empty());
        assert!(!runner.hidden);
    }

    #[tokio::test]
    async fn the_phone_is_still_taken_over_after_coming_back() {
        let client = FakeClient::with(ONLINE, false, 0, None);
        let mut runner = plain(&client);
        runner.tick().await.unwrap();
        assert_eq!(client.presence(), v(&[INVISIBLE]));
        client.f().echo = false;
        client.set_mobile(true);
        client.f().others = 1;
        runner.tick().await.unwrap();
        assert_eq!(client.presence(), v(&[INVISIBLE, ONLINE]));
        assert!(client.calls().is_empty());
        runner.tick().await.unwrap();
        assert!(client.calls().is_empty());
        assert!(!runner.controller.on_mobile);
        client.settle();
        runner.tick().await.unwrap();
        assert_eq!(client.calls(), v(&[IDLE]));
    }

    #[tokio::test]
    async fn a_status_chosen_while_away_is_not_overwritten_on_return() {
        let client = FakeClient::with(ONLINE, false, 0, None);
        client.f().echo = false;
        let mut runner = plain(&client);
        runner.tick().await.unwrap();
        client.f().others = 1;
        runner.tick().await.unwrap();
        client.set_status(DND);
        runner.tick().await.unwrap();
        assert!(!runner.returning);
        assert!(client.calls().is_empty());
    }

    #[tokio::test]
    async fn observe_mode_writes_nothing_while_alone_either_way() {
        let client = FakeClient::with(ONLINE, false, 0, None);
        let mut runner = make(
            &client,
            Opts {
                observe: true,
                ..Opts::default()
            },
        );
        runner.tick().await.unwrap();
        assert!(client.presence().is_empty());
        assert!(runner.hidden);
        client.f().others = 1;
        runner.tick().await.unwrap();
        assert!(client.presence().is_empty());
        assert!(!runner.hidden);
    }

    #[tokio::test]
    async fn choosing_invisible_releases_the_phone_and_hides_us() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        let mut runner = plain(&client);
        runner.tick().await.unwrap();
        assert_eq!(client.calls(), v(&[IDLE]));
        runner.note_settings(INVISIBLE, None);
        runner.tick().await.unwrap();
        assert_eq!(client.calls(), v(&[IDLE]));
        assert_eq!(client.presence(), v(&[INVISIBLE]));
        assert!(!runner.controller.holding);
        assert!(runner.stood_down);
    }

    #[tokio::test]
    async fn offline_counts_the_same_as_invisible() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        let mut runner = plain(&client);
        runner.note_settings(OFFLINE, None);
        runner.tick().await.unwrap();
        assert!(client.calls().is_empty());
        assert_eq!(client.presence(), v(&[INVISIBLE]));
    }

    #[tokio::test]
    async fn standing_down_is_not_written_again() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        let mut runner = plain(&client);
        runner.note_settings(INVISIBLE, None);
        drive(&mut runner, 4).await;
        assert_eq!(client.presence(), v(&[INVISIBLE]));
    }

    #[tokio::test]
    async fn a_phone_arriving_while_invisible_is_ignored() {
        let client = FakeClient::with(INVISIBLE, false, 1, None);
        let mut runner = plain(&client);
        runner.note_settings(INVISIBLE, None);
        runner.tick().await.unwrap();
        client.set_mobile(true);
        drive(&mut runner, 3).await;
        assert!(client.calls().is_empty());
    }

    #[tokio::test]
    async fn being_alone_while_invisible_does_not_arm_the_unhide() {
        let client = FakeClient::with(ONLINE, false, 0, None);
        let mut runner = plain(&client);
        runner.tick().await.unwrap();
        assert!(runner.hidden);
        runner.note_settings(INVISIBLE, None);
        runner.tick().await.unwrap();
        client.f().others = 1;
        runner.tick().await.unwrap();
        assert_eq!(client.presence(), v(&[INVISIBLE, INVISIBLE]));
        assert!(!runner.hidden);
    }

    #[tokio::test]
    async fn our_own_write_is_not_read_as_your_choice() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        let mut runner = plain(&client);
        runner.tick().await.unwrap();
        assert_eq!(client.calls(), v(&[IDLE]));
        runner.note_settings(IDLE, None);
        runner.tick().await.unwrap();
        assert_eq!(runner.user_status, None);
        assert!(!runner.stood_down);
        assert!(runner.controller.holding);
    }

    #[tokio::test]
    async fn a_second_idle_echo_while_we_hold_it_is_not_your_choice() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        let mut runner = plain(&client);
        runner.tick().await.unwrap();
        runner.note_settings(IDLE, None);
        runner.note_settings(IDLE, None);
        assert_eq!(runner.user_status, None);
        assert!(runner.controller.holding);
    }

    #[tokio::test]
    async fn choosing_a_visible_status_again_resumes_management() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        let mut runner = plain(&client);
        runner.note_settings(INVISIBLE, None);
        runner.tick().await.unwrap();
        assert_eq!(client.presence(), v(&[INVISIBLE]));
        runner.note_settings(ONLINE, None);
        runner.tick().await.unwrap();
        assert_eq!(client.presence(), v(&[INVISIBLE, ONLINE]));
        assert!(!runner.stood_down);
        assert_eq!(client.calls(), v(&[IDLE]));
    }

    #[tokio::test]
    async fn the_phone_is_taken_over_once_the_echo_lands() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        client.f().echo = false;
        let mut runner = plain(&client);
        runner.note_settings(INVISIBLE, None);
        runner.tick().await.unwrap();
        client.set_status(INVISIBLE);
        runner.note_settings(ONLINE, None);
        runner.tick().await.unwrap();
        assert_eq!(client.presence(), v(&[INVISIBLE, ONLINE]));
        assert!(client.calls().is_empty());
        runner.tick().await.unwrap();
        assert!(client.calls().is_empty());
        client.settle();
        runner.tick().await.unwrap();
        assert_eq!(client.calls(), v(&[IDLE]));
    }

    #[tokio::test]
    async fn coming_back_uses_the_status_you_chose() {
        let client = FakeClient::new();
        let mut runner = make(
            &client,
            Opts {
                managed: vec![ONLINE, DND],
                ..Opts::default()
            },
        );
        runner.note_settings(INVISIBLE, None);
        runner.tick().await.unwrap();
        runner.note_settings(DND, None);
        runner.tick().await.unwrap();
        assert_eq!(client.presence(), v(&[INVISIBLE, DND]));
    }

    #[tokio::test]
    async fn a_saved_status_is_seeded_at_login() {
        let client = FakeClient::with(ONLINE, true, 1, Some(INVISIBLE));
        let mut runner = plain(&client);
        runner.seed_user_status();
        runner.tick().await.unwrap();
        assert_eq!(runner.user_status, s(INVISIBLE));
        assert!(client.calls().is_empty());
        assert_eq!(client.presence(), v(&[INVISIBLE]));
    }

    #[tokio::test]
    async fn an_unset_saved_status_seeds_nothing() {
        let client = FakeClient::with(ONLINE, true, 1, Some(UNKNOWN));
        let mut runner = plain(&client);
        runner.seed_user_status();
        runner.tick().await.unwrap();
        assert_eq!(runner.user_status, None);
        assert_eq!(client.calls(), v(&[IDLE]));
    }

    #[tokio::test]
    async fn the_wake_fires_so_the_reaction_is_not_a_poll_away() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        let mut runner = plain(&client);
        runner.note_settings(INVISIBLE, None);
        assert!(runner.wake);
    }

    #[tokio::test]
    async fn our_own_echo_does_not_wake_the_loop() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        let mut runner = plain(&client);
        runner.tick().await.unwrap();
        runner.note_settings(IDLE, None);
        assert!(!runner.wake);
    }

    #[tokio::test]
    async fn observe_mode_stands_down_and_up_without_writing() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        let mut runner = make(
            &client,
            Opts {
                observe: true,
                ..Opts::default()
            },
        );
        runner.note_settings(INVISIBLE, None);
        runner.tick().await.unwrap();
        assert!(client.presence().is_empty());
        assert!(runner.stood_down);
        runner.note_settings(ONLINE, None);
        runner.tick().await.unwrap();
        assert!(client.presence().is_empty());
        assert!(!runner.stood_down);
    }

    #[tokio::test]
    async fn a_take_writes_the_hold_marker_and_the_hand_back_clears_it() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        let marker = MemoryMarker::default();
        let mut runner = make(
            &client,
            Opts {
                marker: Some(marker.clone()),
                ..Opts::default()
            },
        );
        runner.tick().await.unwrap();
        assert_eq!(marker.value(), s(ONLINE));
        client.set_mobile(false);
        runner.tick().await.unwrap();
        assert_eq!(marker.value(), None);
        assert_eq!(marker.writes(), vec![s(ONLINE), None]);
    }

    #[tokio::test]
    async fn the_marker_is_only_written_when_it_changes() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        let marker = MemoryMarker::default();
        let mut runner = make(
            &client,
            Opts {
                marker: Some(marker.clone()),
                ..Opts::default()
            },
        );
        drive(&mut runner, 4).await;
        assert_eq!(marker.writes(), vec![s(ONLINE)]);
    }

    #[tokio::test]
    async fn choosing_invisible_mid_hold_clears_the_marker() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        let marker = MemoryMarker::default();
        let mut runner = make(
            &client,
            Opts {
                marker: Some(marker.clone()),
                ..Opts::default()
            },
        );
        runner.tick().await.unwrap();
        runner.note_settings(INVISIBLE, Some(IDLE));
        runner.tick().await.unwrap();
        assert_eq!(marker.value(), None);
    }

    #[tokio::test]
    async fn recovery_restores_once_the_phone_is_gone() {
        let client = FakeClient::with(IDLE, false, 1, Some(IDLE));
        let marker = MemoryMarker::holding(DND);
        let mut runner = make(
            &client,
            Opts {
                managed: vec![ONLINE, DND],
                marker: Some(marker.clone()),
                ..Opts::default()
            },
        );
        runner.recover();
        runner.seed_user_status();
        runner.tick().await.unwrap();
        assert_eq!(runner.user_status, None);
        assert_eq!(client.calls(), v(&[DND]));
        assert_eq!(marker.value(), None);
    }

    #[tokio::test]
    async fn recovery_keeps_holding_while_the_phone_is_still_on() {
        let client = FakeClient::with(IDLE, true, 1, Some(IDLE));
        let marker = MemoryMarker::holding(ONLINE);
        let mut runner = make(
            &client,
            Opts {
                marker: Some(marker),
                ..Opts::default()
            },
        );
        runner.recover();
        drive(&mut runner, 3).await;
        assert!(client.calls().is_empty());
        assert!(runner.controller.holding);
        client.set_mobile(false);
        runner.tick().await.unwrap();
        assert_eq!(client.calls(), v(&[ONLINE]));
    }

    #[tokio::test]
    async fn recovery_with_no_marker_does_nothing() {
        let mut runner = make(
            &FakeClient::new(),
            Opts {
                marker: Some(MemoryMarker::default()),
                ..Opts::default()
            },
        );
        runner.recover();
        assert!(!runner.controller.holding);
    }

    #[tokio::test]
    async fn recovery_is_skipped_in_observe_mode_and_without_a_marker() {
        let mut observed = make(
            &FakeClient::new(),
            Opts {
                observe: true,
                marker: Some(MemoryMarker::holding(ONLINE)),
                ..Opts::default()
            },
        );
        observed.recover();
        let mut bare = plain(&FakeClient::new());
        bare.recover();
        assert!(!observed.controller.holding);
        assert!(!bare.controller.holding);
    }

    #[tokio::test]
    async fn a_status_changed_during_the_restart_is_not_overwritten() {
        let client = FakeClient::with(DND, false, 1, Some(DND));
        let marker = MemoryMarker::holding(ONLINE);
        let mut runner = make(
            &client,
            Opts {
                managed: vec![ONLINE, DND],
                marker: Some(marker.clone()),
                ..Opts::default()
            },
        );
        runner.recover();
        runner.seed_user_status();
        drive(&mut runner, 3).await;
        assert!(client.calls().is_empty());
        assert_eq!(marker.value(), None);
    }

    #[tokio::test]
    async fn ticks_do_nothing_while_disconnected() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        let mut runner = plain(&client);
        runner.note_disconnected();
        drive(&mut runner, 3).await;
        assert!(client.calls().is_empty());
    }

    #[tokio::test]
    async fn a_reconnect_sends_our_presence_again() {
        let client = FakeClient::with(ONLINE, false, 0, None);
        let mut runner = plain(&client);
        runner.tick().await.unwrap();
        assert_eq!(client.presence(), v(&[INVISIBLE]));
        runner.note_disconnected();
        runner.note_connected();
        runner.tick().await.unwrap();
        assert_eq!(client.presence(), v(&[INVISIBLE, INVISIBLE]));
        assert!(!runner.reassert);
    }

    #[tokio::test]
    async fn a_reconnect_before_any_write_sends_nothing() {
        let client = FakeClient::new();
        let mut runner = plain(&client);
        runner.note_connected();
        runner.tick().await.unwrap();
        assert!(client.presence().is_empty());
    }

    #[tokio::test]
    async fn a_second_disconnect_keeps_the_first_timestamp() {
        let o = Opts::default();
        let clock = o.clock.clone();
        let mut runner = make(&FakeClient::new(), o);
        runner.note_disconnected();
        clock.advance(100.0);
        runner.note_disconnected();
        assert_eq!(runner.disconnected_at, Some(1000.0));
    }

    #[tokio::test]
    async fn a_long_disconnect_gives_up() {
        let o = Opts::default();
        let clock = o.clock.clone();
        let mut runner = make(&FakeClient::new(), o);
        runner.note_disconnected();
        clock.advance(DISCONNECT_LIMIT - 1.0);
        runner.tick().await.unwrap();
        assert_eq!(runner.gave_up, None);
        clock.advance(2.0);
        runner.tick().await.unwrap();
        assert!(runner.gave_up.unwrap().contains("no gateway connection"));
    }

    #[tokio::test]
    async fn an_unconfirmed_presence_gives_up_after_the_deadline() {
        let o = Opts {
            echo_seconds: 60.0,
            ..Opts::default()
        };
        let clock = o.clock.clone();
        let client = FakeClient::with(ONLINE, false, 0, None);
        let mut runner = make(&client, o);
        runner.tick().await.unwrap();
        client.f().own = s(ONLINE);
        clock.advance(59.0);
        runner.tick().await.unwrap();
        assert_eq!(runner.gave_up, None);
        clock.advance(2.0);
        runner.tick().await.unwrap();
        assert!(runner.gave_up.unwrap().contains("never confirmed our invisible"));
    }

    #[tokio::test]
    async fn a_confirmed_presence_clears_the_expectation() {
        let client = FakeClient::with(ONLINE, false, 0, None);
        let mut runner = plain(&client);
        runner.tick().await.unwrap();
        client.f().own = s(OFFLINE);
        runner.tick().await.unwrap();
        assert_eq!(runner.expect, None);
    }

    #[tokio::test]
    async fn an_unknown_own_session_is_not_treated_as_deaf() {
        let o = Opts {
            echo_seconds: 1.0,
            ..Opts::default()
        };
        let clock = o.clock.clone();
        let client = FakeClient::with(ONLINE, true, 1, None);
        let mut runner = make(&client, o);
        runner.tick().await.unwrap();
        clock.advance(10.0);
        runner.tick().await.unwrap();
        assert_eq!(runner.gave_up, None);
        assert_eq!(runner.expect, None);
    }

    #[tokio::test]
    async fn a_rejected_write_gives_up() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        client.f().raises = Some(rejected());
        let mut runner = plain(&client);
        runner.cycle().await;
        assert_eq!(
            runner.gave_up.as_deref(),
            Some("Discord rejected the token, it needs replacing")
        );
    }

    #[tokio::test]
    async fn other_write_failures_are_retried() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        client.f().raises = Some(server_error());
        let mut runner = plain(&client);
        runner.cycle().await;
        runner.cycle().await;
        assert_eq!(runner.gave_up, None);
        assert_eq!(client.calls(), v(&[IDLE, IDLE]));
    }

    #[tokio::test]
    async fn the_periodic_token_check_gives_up_on_a_rejected_token() {
        let o = Opts::default();
        let clock = o.clock.clone();
        let client = FakeClient::new();
        client.f().token_error = Some(rejected());
        let mut runner = make(&client, o);
        clock.advance(LIVENESS_SECONDS);
        runner.liveness().await;
        assert_eq!(client.f().token_checks, 1);
        assert!(runner.gave_up.is_some());
    }

    #[tokio::test]
    async fn a_failed_token_check_for_other_reasons_only_warns() {
        let o = Opts::default();
        let clock = o.clock.clone();
        let client = FakeClient::new();
        client.f().token_error = Some(server_error());
        let mut runner = make(&client, o);
        log::take();
        clock.advance(LIVENESS_SECONDS);
        runner.liveness().await;
        assert_eq!(runner.gave_up, None);
        assert!(log::take()
            .iter()
            .any(|(l, m)| *l == log::Level::Warning && m.contains("token check failed")));
    }

    #[tokio::test]
    async fn the_token_check_is_skipped_while_disconnected_or_observing() {
        let o = Opts::default();
        let clock = o.clock.clone();
        let client = FakeClient::new();
        let mut offline = make(
            &client,
            Opts {
                clock: clock.clone(),
                ..Opts::default()
            },
        );
        offline.note_disconnected();
        let mut observing = make(
            &client,
            Opts {
                clock: clock.clone(),
                observe: true,
                ..Opts::default()
            },
        );
        clock.advance(LIVENESS_SECONDS);
        offline.liveness().await;
        observing.liveness().await;
        assert_eq!(client.f().token_checks, 0);
    }

    #[tokio::test]
    async fn a_hanging_write_times_out_and_is_retried() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        let mut runner = make(
            &client,
            Opts {
                write_timeout: 0.01,
                ..Opts::default()
            },
        );
        client.f().hang = true;
        assert!(runner.tick().await.is_err());
        assert_eq!(runner.pending, s(IDLE));
        assert_eq!(runner.presence, None);
    }

    #[tokio::test]
    async fn a_failed_stand_up_is_retried_and_does_not_strand_us_invisible() {
        let client = FakeClient::new();
        let mut runner = plain(&client);
        runner.note_settings(INVISIBLE, Some(ONLINE));
        runner.tick().await.unwrap();
        runner.note_settings(ONLINE, Some(INVISIBLE));
        client.f().unhide_error = Some(server_error());
        assert!(runner.tick().await.is_err());
        assert!(runner.stood_down);
        client.f().unhide_error = None;
        runner.tick().await.unwrap();
        assert!(!runner.stood_down);
        assert_eq!(client.presence(), v(&[INVISIBLE, ONLINE]));
    }

    #[tokio::test]
    async fn coming_back_from_hidden_never_unhides_to_a_hidden_status() {
        let client = FakeClient::with(ONLINE, false, 0, Some(INVISIBLE));
        let mut runner = plain(&client);
        runner.tick().await.unwrap();
        client.f().others = 1;
        runner.tick().await.unwrap();
        assert_eq!(client.presence(), v(&[INVISIBLE, ONLINE]));
    }

    #[tokio::test]
    async fn shutdown_restore_clears_the_marker() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        let marker = MemoryMarker::default();
        let mut runner = make(
            &client,
            Opts {
                marker: Some(marker.clone()),
                ..Opts::default()
            },
        );
        runner.tick().await.unwrap();
        runner.hand_back().await.unwrap();
        assert_eq!(marker.value(), None);
        assert!(!runner.controller.holding);
    }

    #[tokio::test]
    async fn a_failed_shutdown_restore_keeps_the_marker() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        let marker = MemoryMarker::default();
        let mut runner = make(
            &client,
            Opts {
                marker: Some(marker.clone()),
                ..Opts::default()
            },
        );
        runner.tick().await.unwrap();
        client.f().raises = Some(server_error());
        assert!(runner.hand_back().await.is_err());
        assert_eq!(marker.value(), s(ONLINE));
    }

    #[tokio::test]
    async fn shutdown_after_a_manual_override_still_clears_the_marker() {
        let client = FakeClient::with(ONLINE, true, 1, None);
        let marker = MemoryMarker::default();
        let mut runner = make(
            &client,
            Opts {
                marker: Some(marker.clone()),
                grace: 1000.0,
                ..Opts::default()
            },
        );
        runner.tick().await.unwrap();
        client.set_status(DND);
        runner.hand_back().await.unwrap();
        assert_eq!(client.calls(), v(&[IDLE]));
        assert_eq!(marker.value(), None);
    }

    #[tokio::test]
    async fn resume_hold_and_force_put_the_machine_back_in_a_hold() {
        let mut runner = make(
            &FakeClient::new(),
            Opts {
                grace: 25.0,
                ..Opts::default()
            },
        );
        runner.controller.resume_hold(DND);
        let mut debouncer = Debouncer::new(1, 4);
        debouncer.force(true);
        assert!(runner.controller.holding);
        assert!(runner.controller.on_mobile);
        assert_eq!(runner.controller.saved, s(DND));
        assert_eq!(runner.controller.grace_until, 1025.0);
        assert!(debouncer.stable);
        assert!(debouncer.pending);
    }

    #[tokio::test]
    async fn the_first_reason_to_give_up_is_kept() {
        let mut runner = plain(&FakeClient::new());
        runner.give_up("first".into());
        runner.give_up("second".into());
        assert_eq!(runner.gave_up.as_deref(), Some("first"));
    }

    pub struct FakeGateway {
        pub tx: UnboundedSender<Note>,
        pub error: Option<Error>,
        pub finished: bool,
        pub started: u32,
        pub closes: u32,
        pub close_on_ready: bool,
        pub readies: Vec<bool>,
    }

    impl FakeGateway {
        pub fn new(tx: UnboundedSender<Note>) -> Self {
            FakeGateway {
                tx,
                error: None,
                finished: false,
                started: 0,
                closes: 0,
                close_on_ready: false,
                readies: vec![],
            }
        }
    }

    impl Session for FakeGateway {
        fn start(&mut self) {
            self.started += 1;
            if let Some(e) = self.error.clone() {
                let _ = self.tx.send(Note::Ended(Err(e)));
            } else if self.finished {
                let _ = self.tx.send(Note::Ended(Ok(())));
            }
        }
        fn close(&mut self) {
            self.closes += 1;
            let _ = self.tx.send(Note::Ended(Ok(())));
        }
        fn on_ready(&mut self, first: bool) -> bool {
            self.readies.push(first);
            self.close_on_ready
        }
    }

    async fn yield_a_bit() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    #[tokio::test]
    async fn serve_restores_the_saved_status_on_a_stop_signal() {
        let client = FakeClient::with(DND, false, 1, None);
        let mut runner = make(
            &client,
            Opts {
                managed: vec![ONLINE, DND],
                ..Opts::default()
            },
        );
        client.set_mobile(true);
        runner.tick().await.unwrap();
        assert_eq!(client.status(), IDLE);
        let (tx, mut rx) = unbounded_channel();
        let mut gateway = FakeGateway::new(tx);
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let stop = std::pin::pin!(async {
            let _ = stop_rx.await;
        });
        let (code, _) = tokio::join!(serve(&mut runner, &mut gateway, &mut rx, stop, 0), async {
            yield_a_bit().await;
            stop_tx.send(()).unwrap();
        });
        assert_eq!(code, Ok(0));
        assert_eq!(client.status(), DND);
        assert_eq!(gateway.started, 1);
        assert_eq!(gateway.closes, 1);
    }

    #[tokio::test]
    async fn serve_leaves_a_session_that_ended_by_itself_alone() {
        let client = FakeClient::new();
        let mut runner = plain(&client);
        let (tx, mut rx) = unbounded_channel();
        let mut gateway = FakeGateway::new(tx);
        gateway.finished = true;
        let stop = std::pin::pin!(std::future::pending::<()>());
        let code = serve(&mut runner, &mut gateway, &mut rx, stop, 0).await;
        assert_eq!(code, Ok(0));
        assert_eq!(gateway.closes, 0);
        assert!(client.calls().is_empty());
    }

    #[tokio::test]
    async fn serve_propagates_a_failed_login() {
        let mut runner = plain(&FakeClient::new());
        let (tx, mut rx) = unbounded_channel();
        let mut gateway = FakeGateway::new(tx);
        gateway.error = Some(rejected());
        let stop = std::pin::pin!(std::future::pending::<()>());
        let result = serve(&mut runner, &mut gateway, &mut rx, stop, 0).await;
        assert_eq!(result, Err(rejected()));
        assert_eq!(gateway.closes, 0);
    }

    #[tokio::test]
    async fn serve_waits_out_the_start_delay_before_logging_in() {
        tokio::time::pause();
        let mut runner = plain(&FakeClient::new());
        let (tx, mut rx) = unbounded_channel();
        let mut gateway = FakeGateway::new(tx);
        gateway.finished = true;
        let stop = std::pin::pin!(std::future::pending::<()>());
        assert_eq!(serve(&mut runner, &mut gateway, &mut rx, stop, 30).await, Ok(0));
        assert_eq!(gateway.started, 1);
    }

    #[tokio::test]
    async fn a_stop_during_the_start_delay_never_logs_in() {
        let mut runner = plain(&FakeClient::new());
        let (tx, mut rx) = unbounded_channel();
        let mut gateway = FakeGateway::new(tx);
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let stop = std::pin::pin!(async {
            let _ = stop_rx.await;
        });
        let (code, _) = tokio::join!(serve(&mut runner, &mut gateway, &mut rx, stop, 30), async {
            yield_a_bit().await;
            stop_tx.send(()).unwrap();
        });
        assert_eq!(code, Ok(0));
        assert_eq!(gateway.started, 0);
    }

    #[tokio::test]
    async fn serve_abandons_without_restoring_when_the_runner_gives_up() {
        let o = Opts::default();
        let clock = o.clock.clone();
        let client = FakeClient::with(ONLINE, true, 1, None);
        let mut runner = make(&client, o);
        runner.poll_seconds = 30;
        runner.tick().await.unwrap();
        client.f().token_error = Some(rejected());
        clock.advance(LIVENESS_SECONDS);
        let (tx, mut rx) = unbounded_channel();
        tx.send(Note::Ready).unwrap();
        let mut gateway = FakeGateway::new(tx);
        let stop = std::pin::pin!(std::future::pending::<()>());
        let code = serve(&mut runner, &mut gateway, &mut rx, stop, 0).await;
        assert_eq!(code, Ok(1));
        assert_eq!(gateway.closes, 1);
        assert_eq!(client.calls(), v(&[IDLE]));
    }

    #[tokio::test]
    async fn a_session_wake_causes_a_prompt_tick() {
        let client = FakeClient::new();
        let mut runner = plain(&client);
        runner.poll_seconds = 30;
        let (tx, mut rx) = unbounded_channel();
        let mut gateway = FakeGateway::new(tx.clone());
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let stop = std::pin::pin!(async {
            let _ = stop_rx.await;
        });
        let seen = client.clone();
        let (code, calls) = tokio::join!(serve(&mut runner, &mut gateway, &mut rx, stop, 0), async move {
            tx.send(Note::Ready).unwrap();
            yield_a_bit().await;
            let before = seen.calls();
            seen.set_mobile(true);
            tx.send(Note::Sessions).unwrap();
            yield_a_bit().await;
            let after = seen.calls();
            stop_tx.send(()).unwrap();
            (before, after)
        });
        assert_eq!(code, Ok(0));
        assert!(calls.0.is_empty());
        assert_eq!(calls.1, v(&[IDLE]));
        assert_eq!(gateway.readies, vec![true]);
    }

    #[tokio::test]
    async fn a_second_ready_does_not_recover_again() {
        let client = FakeClient::new();
        let marker = MemoryMarker::holding(ONLINE);
        let mut runner = make(
            &client,
            Opts {
                marker: Some(marker.clone()),
                ..Opts::default()
            },
        );
        runner.poll_seconds = 30;
        let (tx, mut rx) = unbounded_channel();
        let mut gateway = FakeGateway::new(tx.clone());
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let stop = std::pin::pin!(async {
            let _ = stop_rx.await;
        });
        let (code, _) = tokio::join!(serve(&mut runner, &mut gateway, &mut rx, stop, 0), async move {
            tx.send(Note::Ready).unwrap();
            yield_a_bit().await;
            tx.send(Note::Ready).unwrap();
            yield_a_bit().await;
            stop_tx.send(()).unwrap();
        });
        assert_eq!(code, Ok(0));
        assert_eq!(gateway.readies, vec![true, false]);
    }

    #[tokio::test]
    async fn the_sessions_dump_closes_on_ready() {
        let mut runner = make(
            &FakeClient::new(),
            Opts {
                observe: true,
                ..Opts::default()
            },
        );
        let (tx, mut rx) = unbounded_channel();
        tx.send(Note::Ready).unwrap();
        let mut gateway = FakeGateway::new(tx);
        gateway.close_on_ready = true;
        let stop = std::pin::pin!(std::future::pending::<()>());
        assert_eq!(serve(&mut runner, &mut gateway, &mut rx, stop, 0).await, Ok(0));
        assert_eq!(gateway.closes, 1);
    }

    struct Dog {
        watchdog: Watchdog,
        now: Arc<AtomicU64>,
        last: Arc<AtomicU64>,
        fired: Arc<AtomicU64>,
    }

    impl Dog {
        fn advance(&self, seconds: f64) {
            let now = f64::from_bits(self.now.load(Ordering::SeqCst)) + seconds;
            self.now.store(now.to_bits(), Ordering::SeqCst);
        }
        fn beat(&self) {
            self.last.store(self.now.load(Ordering::SeqCst), Ordering::SeqCst);
        }
        fn fired(&self) -> u64 {
            self.fired.load(Ordering::SeqCst)
        }
    }

    fn dog(timeout: f64) -> Dog {
        let now = Arc::new(AtomicU64::new(1000f64.to_bits()));
        let last = Arc::new(AtomicU64::new(1000f64.to_bits()));
        let fired = Arc::new(AtomicU64::new(0));
        let clock = now.clone();
        let count = fired.clone();
        let watchdog = Watchdog {
            last_tick: Box::new(beat_reader(last.clone())),
            timeout,
            action: "exiting",
            on_stall: Box::new(move || {
                count.fetch_add(1, Ordering::SeqCst);
            }),
            clock: Box::new(move || f64::from_bits(clock.load(Ordering::SeqCst))),
            fired: false,
        };
        Dog {
            watchdog,
            now,
            last,
            fired,
        }
    }

    #[test]
    fn watchdog_is_quiet_while_ticking() {
        let mut d = dog(100.0);
        for _ in 0..5 {
            d.advance(50.0);
            d.beat();
            assert!(!d.watchdog.check());
        }
        assert_eq!(d.fired(), 0);
    }

    #[test]
    fn watchdog_boundary_is_not_a_stall() {
        let mut d = dog(100.0);
        d.advance(100.0);
        assert!(!d.watchdog.check());
        assert_eq!(d.fired(), 0);
    }

    #[test]
    fn watchdog_fires_once_when_stalled() {
        let mut d = dog(100.0);
        d.advance(101.0);
        assert!(d.watchdog.check());
        assert_eq!(d.fired(), 1);
    }

    #[test]
    fn watchdog_does_not_fire_twice() {
        let mut d = dog(100.0);
        d.advance(500.0);
        assert!(d.watchdog.check());
        assert!(!d.watchdog.check());
        assert_eq!(d.fired(), 1);
    }
}
