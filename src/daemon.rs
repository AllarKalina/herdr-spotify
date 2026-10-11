//! The background process: holds the lock, serves the socket, and runs the player.

mod cover;
mod playback;
pub mod player;
pub mod resume;
pub mod server;
pub mod state;
pub mod web;

use crate::Result;
use crate::config::Config;
use crate::paths::Paths;
use crate::protocol::{Message, Request, State};
use crate::web::catalog::Catalog;
use crate::web::{Api, Endpoints};
use std::fs::{self, File, TryLockError};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UnixListener;
use tokio::sync::{Notify, broadcast, mpsc, watch};

/// How long the daemon stays up with nothing playing and no client connected.
const IDLE: Duration = Duration::from_secs(15 * 60);
/// A daemon being replaced releases the lock a moment after its socket goes away.
const LOCK_WAIT: Duration = Duration::from_secs(4);
const PLAYER_STOP_WAIT: Duration = Duration::from_secs(3);

/// The server's side of the daemon: where requests go and where state and answers come from.
#[derive(Clone)]
pub struct Hub {
    /// Playback requests, for the player.
    pub requests: mpsc::Sender<Request>,
    /// Browsing requests, for the Web API.
    pub web: mpsc::Sender<Request>,
    pub state: watch::Receiver<State>,
    /// Notices and browsing answers, sent to every client.
    pub events: broadcast::Sender<Message>,
}

/// The player's side of the same channels.
pub struct PlayerEnd {
    pub requests: mpsc::Receiver<Request>,
    pub state: watch::Sender<State>,
    pub events: broadcast::Sender<Message>,
    pub stop: Arc<Notify>,
}

/// The Web API's side.
pub struct WebEnd {
    pub requests: mpsc::Receiver<Request>,
    pub events: broadcast::Sender<Message>,
    pub stop: Arc<Notify>,
}

pub fn hub() -> (Hub, PlayerEnd, WebEnd) {
    let (requests_tx, requests_rx) = mpsc::channel(32);
    let (web_tx, web_rx) = mpsc::channel(32);
    let (state_tx, state_rx) = watch::channel(State::default());
    let (events, _) = broadcast::channel(32);
    let hub = Hub {
        requests: requests_tx,
        web: web_tx,
        state: state_rx,
        events: events.clone(),
    };
    let player = PlayerEnd {
        requests: requests_rx,
        state: state_tx,
        events: events.clone(),
        stop: Arc::new(Notify::new()),
    };
    let web = WebEnd {
        requests: web_rx,
        events,
        stop: Arc::new(Notify::new()),
    };
    (hub, player, web)
}

/// Runs until asked to stop, signalled, or idle. Returns at once when a daemon already runs.
pub fn run(paths: &Paths, detached: bool) -> Result<()> {
    if detached {
        // Leave the popup's terminal session, so closing the popup cannot take the music with it.
        let _ = rustix::process::setsid();
    }
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    paths.prepare()?;
    let Some(_lock) = acquire_lock(paths)? else {
        log::info!("another daemon is running; exiting");
        return Ok(());
    };
    log::info!(
        "daemon {} started, pid {}",
        crate::client::build_stamp(),
        std::process::id()
    );

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let outcome = runtime.block_on(serve(paths));
    let _ = fs::remove_file(paths.socket());
    log::info!("daemon stopped");
    outcome
}

async fn serve(paths: &Paths) -> Result<()> {
    let listener = bind(paths)?;
    let (hub, player_end, web_end) = hub();
    let (stop_player, stop_web) = (player_end.stop.clone(), web_end.stop.clone());
    let player = tokio::spawn(player::run(player_end, paths.clone()));
    let api = Api::new(paths.clone(), Config::load()?, Endpoints::from_env())?;
    let web = tokio::spawn(web::run(web_end, Catalog::new(api)));

    let shutdown = Arc::new(Notify::new());
    tokio::spawn(wait_for_signal(shutdown.clone()));
    server::serve(listener, hub, shutdown, idle_period()).await;

    stop_player.notify_one();
    stop_web.notify_one();
    let _ = tokio::time::timeout(PLAYER_STOP_WAIT, player).await;
    web.abort();
    Ok(())
}

fn idle_period() -> Duration {
    // Overridable so tests need not wait fifteen minutes.
    std::env::var("HERDR_SPOTIFY_IDLE_SECS")
        .ok()
        .and_then(|value| value.parse().ok())
        .map_or(IDLE, Duration::from_secs)
}

/// Takes the daemon lock, or returns `None` when another daemon holds it. The lock is tied to
/// the open file, so a crashed daemon can never leave it behind.
fn acquire_lock(paths: &Paths) -> Result<Option<File>> {
    let mut file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(paths.lock())?;
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        match file.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
    }
    file.set_len(0)?;
    writeln!(file, "{}", std::process::id())?;
    Ok(Some(file))
}

/// Binds the socket. Holding the lock means any socket file already there is a leftover.
fn bind(paths: &Paths) -> Result<UnixListener> {
    let socket = paths.socket();
    let _ = fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

async fn wait_for_signal(shutdown: Arc<Notify>) {
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut terminate), Ok(mut interrupt), Ok(mut hangup)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
        signal(SignalKind::hangup()),
    ) else {
        log::error!("cannot listen for signals");
        return;
    };
    loop {
        tokio::select! {
            _ = terminate.recv() => break,
            _ = interrupt.recv() => break,
            // A closing terminal must not stop the music.
            _ = hangup.recv() => log::info!("ignoring hangup"),
        }
    }
    log::info!("signal received");
    shutdown.notify_one();
}
