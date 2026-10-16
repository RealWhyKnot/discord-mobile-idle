use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use super::store;
use crate::config::Config;
use crate::controller::{monotonic, seconds, Debouncer, StatusController, IDLE};
use crate::discord::{self, DiscordClient, Gateway, GatewaySession, Http};
use crate::error;
use crate::persist::{start_delay, HoldMarker};
use crate::runner::{serve, Change, Error, Session, Watchdog};

const BACKOFF_START: u64 = 5;
const BACKOFF_MAX: u64 = 300;
const GRACE_SECONDS: f64 = 25.0;
const STOP_WAIT: Duration = Duration::from_secs(10);

pub type OnState = Arc<dyn Fn(&str, &str) + Send + Sync>;
type Watched = Arc<Mutex<Option<(u64, Arc<AtomicU64>)>>>;

pub enum Control {
    Restart,
    Token(String),
    Stop,
    Stall(u64),
}

pub struct Service {
    controls: UnboundedSender<Control>,
    done: mpsc::Receiver<()>,
}

impl Service {
    pub fn start(config: Config, on_state: OnState) -> Self {
        let (controls, rx) = unbounded_channel();
        let (done_tx, done) = mpsc::channel();
        let watched: Watched = Arc::new(Mutex::new(None));
        start_watchdog(config.watchdog_seconds, watched.clone(), controls.clone());
        std::thread::Builder::new()
            .name("discord".into())
            .spawn(move || {
                Supervisor {
                    token: config.token.clone(),
                    config,
                    on_state,
                    controls: rx,
                    watched,
                    starts: Vec::new(),
                    retry: false,
                    stopping: false,
                    generation: 0,
                }
                .run();
                let _ = done_tx.send(());
            })
            .expect("discord thread");
        Service { controls, done }
    }

    pub fn restart(&self) {
        let _ = self.controls.send(Control::Restart);
    }

    pub fn set_token(&self, token: String) {
        let _ = self.controls.send(Control::Token(token));
    }

    pub fn stop(&self) {
        let _ = self.controls.send(Control::Stop);
        if let Err(mpsc::RecvTimeoutError::Timeout) = self.done.recv_timeout(STOP_WAIT) {
            error!("shutdown did not finish cleanly");
        }
    }
}

fn locked<T>(slot: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    slot.lock().unwrap_or_else(|e| e.into_inner())
}

fn start_watchdog(timeout: u64, watched: Watched, controls: UnboundedSender<Control>) {
    let reader = watched.clone();
    let mut dog = Watchdog {
        last_tick: Box::new(move || match &*locked(&reader) {
            Some((_, beat)) => f64::from_bits(beat.load(Ordering::Relaxed)),
            None => seconds(),
        }),
        timeout: timeout as f64,
        action: "reconnecting",
        on_stall: Box::new(move || {
            if let Some((generation, _)) = locked(&watched).take() {
                let _ = controls.send(Control::Stall(generation));
            }
        }),
        clock: Box::new(seconds),
        fired: false,
    };
    let interval = Duration::from_secs_f64((timeout as f64 / 3.0).max(1.0));
    std::thread::Builder::new()
        .name("watchdog".into())
        .stack_size(64 * 1024)
        .spawn(move || loop {
            std::thread::sleep(interval);
            if dog.check() {
                dog.fired = false;
            }
        })
        .expect("watchdog thread");
}

struct TraySession {
    inner: GatewaySession,
    on_state: OnState,
    watched: Watched,
    generation: u64,
    beat: Arc<AtomicU64>,
}

impl Session for TraySession {
    fn start(&mut self) {
        self.inner.start();
    }

    fn close(&mut self) {
        self.inner.close();
    }

    fn on_ready(&mut self, first: bool) -> bool {
        let user = self.inner.link.borrow().user.clone();
        (self.on_state)("active", &format!("connected as {}", user));
        let close = self.inner.on_ready(first);
        if first {
            *locked(&self.watched) = Some((self.generation, self.beat.clone()));
        }
        close
    }
}

struct Supervisor {
    config: Config,
    token: String,
    on_state: OnState,
    controls: UnboundedReceiver<Control>,
    watched: Watched,
    starts: Vec<f64>,
    retry: bool,
    stopping: bool,
    generation: u64,
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

impl Supervisor {
    fn state(&self, state: &str, detail: &str) {
        (self.on_state)(state, detail);
    }

    fn run(mut self) {
        let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(runtime) => runtime,
            Err(e) => {
                error!("could not start the runtime: {}", e);
                return;
            }
        };
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, self.supervise());
    }

    fn take(&mut self, control: Option<Control>) {
        match control {
            Some(Control::Restart) => self.retry = true,
            Some(Control::Token(token)) => {
                self.token = token;
                self.retry = true;
            }
            Some(Control::Stop) | None => {
                self.stopping = true;
                self.retry = true;
            }
            Some(Control::Stall(_)) => {}
        }
    }

    async fn wait(&mut self, limit: Option<u64>) -> bool {
        loop {
            if self.retry {
                self.retry = false;
                return true;
            }
            let control = match limit {
                Some(limit) => match tokio::time::timeout(Duration::from_secs(limit), self.controls.recv()).await {
                    Ok(control) => control,
                    Err(_) => return false,
                },
                None => self.controls.recv().await,
            };
            self.take(control);
        }
    }

    async fn supervise(&mut self) {
        let mut delay = BACKOFF_START;
        while !self.stopping {
            let (pause, starts) = start_delay(&self.starts, now());
            self.starts = starts;
            if pause > 0 {
                self.state("connecting", &format!("reconnected too often, waiting {}s", pause));
                self.wait(Some(pause)).await;
                if self.stopping {
                    return;
                }
            }
            self.starts.push(now());
            self.state("connecting", "connecting");
            match self.session().await {
                Ok(_) => delay = BACKOFF_START,
                Err(e) if e.rejected() => {
                    self.state("error", "token rejected, set a new one");
                    self.wait(None).await;
                    delay = BACKOFF_START;
                    continue;
                }
                Err(e) => error!("session ended: {}", e),
            }
            if self.stopping {
                return;
            }
            self.state("connecting", &format!("reconnecting in {}s", delay));
            if self.wait(Some(delay)).await {
                delay = BACKOFF_START;
            } else {
                delay = (delay * 2).min(BACKOFF_MAX);
            }
        }
    }

    async fn session(&mut self) -> Result<i32, Error> {
        let http = Rc::new(Http::new(self.token.clone())?);
        let link = discord::link();
        let (notes_tx, mut notes) = unbounded_channel();
        let (commands_tx, commands_rx) = unbounded_channel();
        let client = DiscordClient {
            link: link.clone(),
            http: http.clone(),
            commands: commands_tx.clone(),
        };
        let config = &self.config;
        let controller = StatusController::new(&config.restore, config.managed.clone(), GRACE_SECONDS, monotonic());
        let debouncer = Debouncer::new(config.on_polls, config.off_polls);
        let mut runner = crate::runner::Runner::new(
            client,
            controller,
            debouncer,
            config.poll_seconds,
            false,
            monotonic(),
            Some(HoldMarker::new(store::path_for("hold"))),
        );
        let stats_link = link.clone();
        runner.stats = Some(Box::new(move || stats_link.borrow().state.ignored_summary(8)));
        runner.games = config.game_dnd;
        runner.game_ignore = config.game_ignore.clone();
        runner.game_marker = Some(HoldMarker::new(store::path_for("game-hold")));
        let on_state = self.on_state.clone();
        runner.on_change = Some(Box::new(move |change| match change {
            Change::Wrote(status) if status == IDLE => on_state("holding", "idle while your phone is awake"),
            Change::Playing => on_state("holding", "do not disturb while you play"),
            Change::Wrote(status) | Change::Shown(status) => on_state("active", &format!("back to {}", status)),
            Change::Hidden { by_choice: true } => on_state("active", "leaving your status alone"),
            Change::Hidden { by_choice: false } => on_state("active", "offline while nothing else is connected"),
        }));
        self.generation += 1;
        let generation = self.generation;
        let mut session = TraySession {
            inner: GatewaySession {
                gateway: Some(Gateway::new(http, link.clone(), notes_tx, commands_rx)),
                commands: commands_tx,
                link,
                sessions_only: false,
                ready_file: None,
                watchdog: None,
            },
            on_state: self.on_state.clone(),
            watched: self.watched.clone(),
            generation,
            beat: runner.beat.clone(),
        };
        let on_state = self.on_state.clone();
        let result = {
            let controls = &mut self.controls;
            let token = &mut self.token;
            let retry = &mut self.retry;
            let stopping = &mut self.stopping;
            let stop = std::pin::pin!(async move {
                loop {
                    match controls.recv().await {
                        Some(Control::Restart) => *retry = true,
                        Some(Control::Token(new)) => {
                            *token = new;
                            *retry = true;
                        }
                        Some(Control::Stop) | None => {
                            *stopping = true;
                            *retry = true;
                        }
                        Some(Control::Stall(seen)) if seen == generation => {
                            error!("watchdog fired, rebuilding the session");
                            on_state("connecting", "recovering");
                        }
                        Some(Control::Stall(_)) => continue,
                    }
                    return;
                }
            });
            serve(&mut runner, &mut session, &mut notes, stop, 0).await
        };
        {
            let mut watched = locked(&self.watched);
            if watched.as_ref().is_some_and(|(seen, _)| *seen == generation) {
                *watched = None;
            }
        }
        if let (Ok(1), Some(reason)) = (&result, &runner.gave_up) {
            self.state("connecting", reason);
        }
        result
    }
}
