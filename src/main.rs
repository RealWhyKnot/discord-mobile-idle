mod config;
mod controller;
mod discord;
mod log;
mod persist;
mod proto;
mod runner;
mod state;

use std::path::PathBuf;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc::unbounded_channel;

use crate::config::{load_config, py_list};
use crate::controller::{monotonic, Debouncer, StatusController};
use crate::discord::{DiscordClient, Gateway, GatewaySession, Http};
use crate::persist::{start_delay, HoldMarker, StartLog};
use crate::runner::{serve, Runner};

const GRACE_SECONDS: f64 = 25.0;
const USAGE: &str = "usage: idlebot [-h] [--observe] [--sessions]";

struct Args {
    observe: bool,
    sessions: bool,
}

fn parse_args() -> Result<Args, i32> {
    let mut args = Args {
        observe: false,
        sessions: false,
    };
    let mut unknown = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--observe" => args.observe = true,
            "--sessions" => args.sessions = true,
            "-h" | "--help" => {
                println!("{}\n\noptions:\n  -h, --help  show this help message and exit\n  --observe   log intended changes, write nothing\n  --sessions  dump sessions once and exit", USAGE);
                return Err(0);
            }
            _ => unknown.push(arg),
        }
    }
    if !unknown.is_empty() {
        eprintln!(
            "{}\nidlebot: error: unrecognized arguments: {}",
            USAGE,
            unknown.join(" ")
        );
        return Err(2);
    }
    Ok(args)
}

fn env_path(name: &str, default: PathBuf) -> PathBuf {
    std::env::var_os(name).map(PathBuf::from).unwrap_or(default)
}

fn py_bool(value: bool) -> &'static str {
    if value {
        "True"
    } else {
        "False"
    }
}

#[cfg(unix)]
async fn stop_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    match (signal(SignalKind::interrupt()), signal(SignalKind::terminate())) {
        (Ok(mut interrupt), Ok(mut terminate)) => {
            tokio::select! {
                _ = interrupt.recv() => {}
                _ = terminate.recv() => {}
            }
        }
        _ => {
            warning!("could not install signal handlers");
            std::future::pending::<()>().await
        }
    }
}

#[cfg(not(unix))]
async fn stop_signal() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await
    }
}

fn run() -> i32 {
    let args = match parse_args() {
        Ok(args) => args,
        Err(code) => return code,
    };
    let config = match load_config(&|name| std::env::var(name).ok()) {
        Ok(config) => config,
        Err(e) => {
            error!("configuration error: {}", e);
            return 2;
        }
    };
    let state_dir = env_path("STATE_DIR", std::env::temp_dir());
    let ready_file = env_path("READY_FILE", PathBuf::from("/tmp/ready"));

    let http = match Http::new(config.token.clone()) {
        Ok(http) => Rc::new(http),
        Err(e) => {
            error!("{}", e);
            return 1;
        }
    };
    let link = discord::link();
    let (notes_tx, mut notes) = unbounded_channel();
    let (commands_tx, commands_rx) = unbounded_channel();
    let client = DiscordClient {
        link: link.clone(),
        http: http.clone(),
        commands: commands_tx.clone(),
    };
    let controller = StatusController::new(&config.restore, config.managed.clone(), GRACE_SECONDS, monotonic());
    let debouncer = Debouncer::new(config.on_polls, config.off_polls);
    let mut runner = Runner::new(
        client,
        controller,
        debouncer,
        config.poll_seconds,
        args.observe || args.sessions,
        monotonic(),
        Some(HoldMarker::new(state_dir.join("idlebot-hold"))),
    );
    let stats_link = link.clone();
    runner.stats = Some(Box::new(move || stats_link.borrow().state.ignored_summary(8)));
    let mut session = GatewaySession {
        gateway: Some(Gateway::new(http, link.clone(), notes_tx, commands_rx)),
        commands: commands_tx,
        link,
        sessions_only: args.sessions,
        ready_file,
        watchdog: Some((runner.beat.clone(), config.watchdog_seconds)),
    };

    let starts = StartLog::new(state_dir.join("idlebot-starts"));
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let (delay, mut recent) = start_delay(&starts.load(), now);
    let stamp = now + delay as f64;
    recent.push(stamp);
    starts.save(&recent);

    info!(
        "starting observe={} poll={}s managed={} restore={}",
        py_bool(args.observe),
        config.poll_seconds,
        py_list(config.managed.iter().map(String::as_str)),
        config.restore
    );

    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(e) => {
            error!("could not start the runtime: {}", e);
            return 1;
        }
    };
    let local = tokio::task::LocalSet::new();
    let result = local.block_on(&runtime, async {
        let stop = std::pin::pin!(stop_signal());
        serve(&mut runner, &mut session, &mut notes, stop, delay).await
    });
    match result {
        Ok(code) => {
            if code == 0 {
                starts.forget(stamp);
            }
            code
        }
        Err(e) if e.rejected() => {
            error!("login failed: DISCORD_TOKEN was rejected");
            1
        }
        Err(e) => {
            error!("the Discord session ended with an error: {}", e);
            1
        }
    }
}

fn main() {
    std::process::exit(run());
}
