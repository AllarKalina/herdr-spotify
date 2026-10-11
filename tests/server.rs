//! The socket server against a stand-in player: what clients are sent, and when.

use herdr_spotify::daemon::{self, PlayerEnd, server};
use herdr_spotify::protocol::{Message, PROTOCOL, Request, State, Status};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::timeout;

static NEXT: AtomicU32 = AtomicU32::new(0);
const SOON: Duration = Duration::from_secs(3);
const NEVER: Duration = Duration::from_secs(3600);

struct Server {
    socket: PathBuf,
    shutdown: Arc<Notify>,
    task: JoinHandle<()>,
}

/// A player that reacts to requests the way the real one reports them, minus Spotify.
async fn stand_in_player(mut end: PlayerEnd) {
    end.state.send_modify(|state| state.status = Status::Ready);
    while let Some(request) = end.requests.recv().await {
        match request {
            Request::Play => end.state.send_modify(|state| state.playing = true),
            Request::Pause => end.state.send_modify(|state| state.playing = false),
            Request::Volume { percent } => end.state.send_modify(|state| state.volume = percent),
            Request::Next => {
                let _ = end.notices.send("Track unavailable".into());
            }
            _ => {}
        }
    }
}

fn start(idle: Duration) -> Server {
    let name = format!(
        "hs-server-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    );
    let socket = PathBuf::from("/tmp").join(name);
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).expect("bind");
    let (hub, end) = daemon::hub();
    tokio::spawn(stand_in_player(end));
    let shutdown = Arc::new(Notify::new());
    let task = tokio::spawn(server::serve(listener, hub, shutdown.clone(), idle));
    Server {
        socket,
        shutdown,
        task,
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown.notify_one();
        let _ = std::fs::remove_file(&self.socket);
    }
}

struct Client {
    lines: Lines<BufReader<OwnedReadHalf>>,
    write: OwnedWriteHalf,
}

impl Client {
    /// Connects and consumes the greeting; returns the first state too.
    async fn connect(server: &Server) -> (Self, State) {
        let (read, write) = UnixStream::connect(&server.socket)
            .await
            .expect("connect")
            .into_split();
        let mut client = Self {
            lines: BufReader::new(read).lines(),
            write,
        };
        match client.next().await {
            Message::Hello { protocol, pid, .. } => {
                assert_eq!(protocol, PROTOCOL);
                assert_eq!(pid, std::process::id());
            }
            other => panic!("expected hello, got {other:?}"),
        }
        let Message::State(state) = client.next().await else {
            panic!("expected state")
        };
        (client, state)
    }

    async fn next(&mut self) -> Message {
        let line = timeout(SOON, self.lines.next_line())
            .await
            .expect("the daemon said nothing")
            .expect("read")
            .expect("the daemon hung up");
        serde_json::from_str(&line).expect("a protocol message")
    }

    async fn state(&mut self) -> State {
        match self.next().await {
            Message::State(state) => state,
            other => panic!("expected state, got {other:?}"),
        }
    }

    async fn send_line(&mut self, line: &str) {
        self.write
            .write_all(format!("{line}\n").as_bytes())
            .await
            .expect("write");
    }

    async fn send(&mut self, request: &Request) {
        self.send_line(&serde_json::to_string(request).expect("encode"))
            .await;
    }

    async fn is_quiet(&mut self) -> bool {
        timeout(Duration::from_millis(200), self.lines.next_line())
            .await
            .is_err()
    }
}

#[tokio::test]
async fn every_client_gets_the_state_on_connect_and_each_change_after() {
    let server = start(NEVER);
    let (mut one, _) = Client::connect(&server).await;
    let (mut two, _) = Client::connect(&server).await;

    one.send(&Request::Play).await;
    assert!(one.state().await.playing);
    assert!(two.state().await.playing);

    two.send(&Request::Volume { percent: 30 }).await;
    assert_eq!(one.state().await.volume, 30);
    assert_eq!(two.state().await.volume, 30);

    let (_late, state) = Client::connect(&server).await;
    assert_eq!(
        (state.status, state.playing, state.volume),
        (Status::Ready, true, 30)
    );
}

#[tokio::test]
async fn nothing_is_sent_while_nothing_changes() {
    let server = start(NEVER);
    let (mut client, _) = Client::connect(&server).await;
    assert!(client.is_quiet().await);
}

#[tokio::test]
async fn a_malformed_request_gets_a_notice_and_the_connection_stays_usable() {
    let server = start(NEVER);
    let (mut client, _) = Client::connect(&server).await;
    let (mut other, _) = Client::connect(&server).await;

    client.send_line("{\"type\":\"dance\"}").await;
    match client.next().await {
        Message::Notice { text } => assert!(text.starts_with("Unrecognised request"), "{text}"),
        message => panic!("expected a notice, got {message:?}"),
    }
    assert!(
        other.is_quiet().await,
        "one client's mistake is not another's news"
    );

    client.send(&Request::Play).await;
    assert!(client.state().await.playing);
}

#[tokio::test]
async fn player_notices_reach_clients() {
    let server = start(NEVER);
    let (mut client, _) = Client::connect(&server).await;
    client.send(&Request::Next).await;
    assert_eq!(
        client.next().await,
        Message::Notice {
            text: "Track unavailable".into()
        }
    );
}

#[tokio::test]
async fn a_shutdown_request_stops_the_server() {
    let mut server = start(NEVER);
    let (mut client, _) = Client::connect(&server).await;
    client.send(&Request::Shutdown).await;
    timeout(SOON, &mut server.task)
        .await
        .expect("the server should stop")
        .expect("join");
}

#[tokio::test]
async fn idle_means_nothing_playing_and_nobody_connected() {
    let idle = Duration::from_millis(300);

    // A connected client keeps it up.
    let mut server = start(idle);
    let (mut client, _) = Client::connect(&server).await;
    assert!(timeout(idle * 4, &mut server.task).await.is_err());

    // So does playback, after the client has gone.
    client.send(&Request::Play).await;
    assert!(client.state().await.playing);
    drop(client);
    assert!(timeout(idle * 4, &mut server.task).await.is_err());

    // Paused and alone, it goes.
    let (mut client, _) = Client::connect(&server).await;
    client.send(&Request::Pause).await;
    assert!(!client.state().await.playing);
    drop(client);
    timeout(SOON, &mut server.task)
        .await
        .expect("the server should exit when idle")
        .expect("join");
}
