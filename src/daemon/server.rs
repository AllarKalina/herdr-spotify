//! The socket: one task per client, each given the state now and every change after it.

use super::Hub;
use crate::Result;
use crate::protocol::{Message, PROTOCOL, Request, encode};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Notify;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::Instant;

/// Serves clients until `shutdown` fires or the daemon has been idle for `idle`: nothing
/// playing and nobody connected.
pub async fn serve(listener: UnixListener, hub: Hub, shutdown: Arc<Notify>, idle: Duration) {
    let clients = Arc::new(AtomicUsize::new(0));
    let mut idle_since = Instant::now();
    let mut tick =
        tokio::time::interval((idle / 4).clamp(Duration::from_millis(50), Duration::from_secs(5)));
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let (hub, shutdown, clients) = (hub.clone(), shutdown.clone(), clients.clone());
                    clients.fetch_add(1, Ordering::SeqCst);
                    tokio::spawn(async move {
                        if let Err(error) = client(stream, hub, &shutdown).await {
                            log::debug!("client ended: {error}");
                        }
                        clients.fetch_sub(1, Ordering::SeqCst);
                    });
                }
                Err(error) => {
                    log::error!("socket accept failed: {error}");
                    break;
                }
            },
            () = shutdown.notified() => break,
            _ = tick.tick() => {
                if hub.state.borrow().playing || clients.load(Ordering::SeqCst) > 0 {
                    idle_since = Instant::now();
                } else if idle_since.elapsed() >= idle {
                    log::info!("idle for {}s; exiting", idle.as_secs());
                    break;
                }
            }
        }
    }
}

async fn client(stream: UnixStream, hub: Hub, shutdown: &Notify) -> Result<()> {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    let mut state = hub.state.clone();
    let mut events = hub.events.subscribe();

    let hello = Message::Hello {
        protocol: PROTOCOL,
        build: crate::client::build_stamp(),
        pid: std::process::id(),
    };
    send(&mut write, &hello).await?;
    let snapshot = state.borrow_and_update().clone();
    send(&mut write, &Message::State(snapshot)).await?;

    loop {
        tokio::select! {
            changed = state.changed() => {
                if changed.is_err() {
                    break;
                }
                let snapshot = state.borrow_and_update().clone();
                send(&mut write, &Message::State(snapshot)).await?;
            }
            event = events.recv() => match event {
                Ok(message) => send(&mut write, &message).await?,
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            },
            line = lines.next_line() => {
                let Some(line) = line? else { break };
                match serde_json::from_str::<Request>(&line) {
                    Ok(Request::Shutdown) => {
                        log::info!("shutdown requested by a client");
                        shutdown.notify_one();
                        break;
                    }
                    Ok(request) => {
                        let browsing = matches!(
                            request,
                            Request::GetLibrary { .. } | Request::GetTracks { .. } | Request::Search { .. }
                        );
                        let target = if browsing { &hub.web } else { &hub.requests };
                        if target.send(request).await.is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let text = format!("Unrecognised request: {error}");
                        send(&mut write, &Message::Notice { text }).await?;
                    }
                }
            }
        }
    }
    Ok(())
}

async fn send(write: &mut OwnedWriteHalf, message: &Message) -> Result<()> {
    write.write_all(encode(message)?.as_bytes()).await?;
    Ok(())
}
