//! The Spotify side: a librespot Connect device that plays on this computer.

use super::PlayerEnd;
use super::state::{self, Change};
use crate::paths::Paths;
use crate::protocol::{Repeat, Request, Status, Track, now_ms};
use crate::{Result, credentials, login};
use librespot_connect::{ConnectConfig, LoadRequest, LoadRequestOptions, PlayingTrack, Spirc};
use librespot_core::{authentication::Credentials, config::SessionConfig, session::Session};
use librespot_metadata::audio::{AudioItem, UniqueFields};
use librespot_playback::audio_backend;
use librespot_playback::config::{AudioFormat, PlayerConfig};
use librespot_playback::mixer::{self, MixerConfig};
use librespot_playback::player::{Player, PlayerEvent, PlayerEventChannel};
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

/// A session that lasted this long and then dropped is reconnected without being asked.
const STABLE_SESSION: Duration = Duration::from_secs(30);
const SPIRC_STOP_WAIT: Duration = Duration::from_secs(2);
const SIGN_IN_HINT: &str = "Not signed in. Run: herdr-spotify login";

enum Failure {
    SignedOut,
    Unavailable(String),
}

enum Ended {
    /// The daemon is stopping.
    Stop,
    /// Spotify dropped the session.
    Lost,
}

struct Connected {
    spirc: Spirc,
    task: Pin<Box<dyn Future<Output = ()> + Send>>,
    events: PlayerEventChannel,
}

/// Keeps a session up for as long as the daemon runs. Without one, each request is a reason
/// to try again; nothing retries on a timer.
pub async fn run(mut end: PlayerEnd, paths: Paths) {
    let mut pending: Option<Request> = None;
    loop {
        change(&end, Change::Status(Status::Connecting, None));
        let retry_now = match connect(&paths).await {
            Ok(mut connected) => {
                change(&end, Change::Status(Status::Ready, None));
                if let Some(request) = pending.take() {
                    handle(&connected.spirc, request, &end);
                }
                let started = Instant::now();
                match drive(&mut connected, &mut end).await {
                    Ended::Stop => return,
                    Ended::Lost => {
                        log::warn!("Spotify session ended");
                        let problem = "The Spotify session ended".to_owned();
                        change(&end, Change::Status(Status::Unavailable, Some(problem)));
                        started.elapsed() >= STABLE_SESSION
                    }
                }
            }
            Err(failure) => {
                let (status, problem) = match failure {
                    Failure::SignedOut => (Status::SignedOut, SIGN_IN_HINT.to_owned()),
                    Failure::Unavailable(problem) => (Status::Unavailable, problem),
                };
                log::warn!("no Spotify session: {problem}");
                if pending.take().is_some() {
                    let _ = end.notices.send(problem.clone());
                }
                let problem = (status == Status::Unavailable).then_some(problem);
                change(&end, Change::Status(status, problem));
                false
            }
        };
        if retry_now {
            continue;
        }
        tokio::select! {
            () = end.stop.notified() => return,
            request = end.requests.recv() => match request {
                Some(request) => pending = Some(request),
                None => return,
            },
        }
    }
}

async fn connect(paths: &Paths) -> std::result::Result<Connected, Failure> {
    let unavailable = |error: &dyn std::fmt::Display| Failure::Unavailable(error.to_string());

    let Some(refresh_token) = credentials::audio(paths).map_err(|e| unavailable(&e))? else {
        return Err(Failure::SignedOut);
    };
    let session_config = SessionConfig {
        device_id: device_id(paths).map_err(|e| unavailable(&e))?,
        ..SessionConfig::default()
    };
    let token = login::oauth_client(&session_config.client_id, false)
        .map_err(|e| unavailable(&e))?
        .refresh_token_async(&refresh_token)
        .await
        .map_err(|error| {
            let text = error.to_string();
            if text.contains("invalid_grant") {
                // Spotify revoked the sign-in; only a new login helps.
                let _ = credentials::forget_audio(paths);
                Failure::SignedOut
            } else {
                Failure::Unavailable(format!("Cannot reach Spotify: {text}"))
            }
        })?;
    if !token.refresh_token.is_empty() && token.refresh_token != refresh_token {
        credentials::set_audio(paths, &token.refresh_token).map_err(|e| unavailable(&e))?;
    }

    let session = Session::new(session_config, None);
    let mixer =
        mixer::find(None).ok_or_else(|| unavailable(&"No audio mixer"))?(MixerConfig::default())
            .map_err(|e| unavailable(&e))?;
    let sink = audio_backend::find(None).ok_or_else(|| unavailable(&"No audio output"))?;
    let player = Player::new(
        PlayerConfig::default(),
        session.clone(),
        mixer.get_soft_volume(),
        move || sink(None, AudioFormat::default()),
    );
    let events = player.get_player_event_channel();
    let config = ConnectConfig {
        name: device_name(),
        ..ConnectConfig::default()
    };
    let credentials = Credentials::with_access_token(token.access_token);
    let (spirc, task) = Spirc::new(config, session.clone(), credentials, player, mixer)
        .await
        .map_err(|error| Failure::Unavailable(format!("Spotify refused the session: {error}")))?;
    log::info!("connected to Spotify as {}", session.username());
    Ok(Connected {
        spirc,
        task: Box::pin(task),
        events,
    })
}

/// Runs one session: requests in, player events out.
async fn drive(connected: &mut Connected, end: &mut PlayerEnd) -> Ended {
    loop {
        tokio::select! {
            () = &mut connected.task => return Ended::Lost,
            () = end.stop.notified() => break,
            request = end.requests.recv() => match request {
                Some(request) => handle(&connected.spirc, request, end),
                None => break,
            },
            event = connected.events.recv() => match event {
                Some(event) => report(event, end),
                None => return Ended::Lost,
            },
        }
    }
    let _ = connected.spirc.shutdown();
    let _ = tokio::time::timeout(SPIRC_STOP_WAIT, &mut connected.task).await;
    Ended::Stop
}

fn handle(spirc: &Spirc, request: Request, end: &PlayerEnd) {
    let outcome = match request {
        Request::Play => spirc.play(),
        Request::Pause => spirc.pause(),
        Request::Toggle => spirc.play_pause(),
        Request::Next => spirc.next(),
        Request::Previous => spirc.prev(),
        Request::Seek { position_ms } => spirc.set_position_ms(position_ms),
        Request::Volume { percent } => spirc.set_volume(state::volume_raw(percent)),
        Request::Shuffle { on } => spirc.shuffle(on),
        Request::Repeat { mode } => spirc
            .repeat(mode == Repeat::Context)
            .and_then(|()| spirc.repeat_track(mode == Repeat::Track)),
        Request::PlayContext { uri, index } => {
            let options = LoadRequestOptions {
                start_playing: true,
                playing_track: index.map(PlayingTrack::Index),
                ..LoadRequestOptions::default()
            };
            change(end, Change::Context(Some(uri.clone())));
            spirc
                .activate()
                .and_then(|()| spirc.load(LoadRequest::from_context_uri(uri, options)))
        }
        // The server answers this one itself.
        Request::Shutdown => Ok(()),
    };
    if let Err(error) = outcome {
        log::warn!("request failed: {error}");
        let _ = end.notices.send(format!("Spotify request failed: {error}"));
    }
}

fn report(event: PlayerEvent, end: &PlayerEnd) {
    let reported = match event {
        PlayerEvent::TrackChanged { audio_item } => Change::Track(track(&audio_item)),
        PlayerEvent::Playing { position_ms, .. } => Change::Playing { position_ms },
        PlayerEvent::Paused { position_ms, .. } => Change::Paused { position_ms },
        PlayerEvent::Seeked { position_ms, .. }
        | PlayerEvent::PositionCorrection { position_ms, .. } => Change::Position { position_ms },
        PlayerEvent::Stopped { .. } => Change::Stopped,
        PlayerEvent::VolumeChanged { volume } => Change::Volume(state::volume_percent(volume)),
        PlayerEvent::ShuffleChanged { shuffle } => Change::Shuffle(shuffle),
        PlayerEvent::RepeatChanged { context, track } => {
            Change::Repeat(state::repeat(context, track))
        }
        PlayerEvent::Unavailable { track_id, .. } => {
            let _ = end.notices.send(format!("Track unavailable: {track_id}"));
            return;
        }
        _ => return,
    };
    change(end, reported);
}

fn change(end: &PlayerEnd, change: Change) {
    end.state
        .send_if_modified(|current| state::apply(current, change, now_ms()));
}

fn track(item: &AudioItem) -> Track {
    let (artists, album) = match &item.unique_fields {
        UniqueFields::Track { artists, album, .. } => (
            artists.iter().map(|artist| artist.name.clone()).collect(),
            album.clone(),
        ),
        UniqueFields::Local { artists, album, .. } => (
            artists.iter().cloned().collect(),
            album.clone().unwrap_or_default(),
        ),
        UniqueFields::Episode { show_name, .. } => (vec![show_name.clone()], String::new()),
    };
    Track {
        uri: item.uri.clone(),
        name: item.name.clone(),
        artists,
        album,
        duration_ms: item.duration_ms,
        // Covers come largest first; the smallest is plenty for a terminal.
        cover_url: item.covers.last().map(|cover| cover.url.clone()),
    }
}

/// The device ID Spotify knows this computer by, created on first use.
fn device_id(paths: &Paths) -> Result<String> {
    let path = paths.device_id();
    if let Ok(id) = std::fs::read_to_string(&path) {
        let id = id.trim();
        if !id.is_empty() {
            return Ok(id.to_owned());
        }
    }
    let id = SessionConfig::default().device_id;
    std::fs::write(&path, &id)?;
    Ok(id)
}

fn device_name() -> String {
    let name = std::process::Command::new("scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Mac".into());
    format!("Herdr · {name}")
}
