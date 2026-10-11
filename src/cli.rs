//! The command line: run or stop the daemon, sign in, and drive playback from a shell.

use crate::client::{self, Connection};
use crate::paths::Paths;
use crate::protocol::{LIKED, Message, Repeat, Request, State, Status, Track, now_ms};
use crate::web::catalog::LIBRARY_TTL_MS;
use crate::{Result, daemon, login, ui};
use std::time::{Duration, Instant};

const USAGE: &str = "\
Usage: herdr-spotify <command>

  open                        Open the player popup in Herdr
  tui                         Run the player interface in this terminal
  login                       Sign in to Spotify on this computer: audio, then browsing
  status [--json]             Show what is playing (starts the daemon if needed)
  ctl <action>                Control playback:
        play | pause | toggle | next | previous
        seek <seconds> | volume <0-100> | shuffle <on|off> | repeat <off|context|track>
        play-context <spotify-uri> [track-index | track-uri]
        play-tracks <index> <track-uri>...
  library [--refresh]         List your playlists
  tracks <playlist-uri|liked> [--more]
                              List the tracks loaded so far, or load 50 more
  search <words>              Search tracks and playlists
  daemon                      Run the player in the foreground
  daemon stop                 Stop the background player";

/// How long `ctl` waits for the daemon to report the effect of a request.
const EFFECT_WAIT: Duration = Duration::from_secs(6);
/// How long the daemon must stay quiet before `ctl` takes the state as settled.
const SETTLE_WAIT: Duration = Duration::from_millis(400);
const LOAD_WAIT: Duration = Duration::from_millis(2500);

pub fn run(args: Vec<String>) -> Result<()> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        [] | ["help" | "--help" | "-h"] => {
            println!("{USAGE}");
            Ok(())
        }
        ["daemon"] => daemon::run(&Paths::from_env()?, false),
        ["daemon", "--detached"] => daemon::run(&Paths::from_env()?, true),
        ["daemon", "stop"] => {
            let stopped = client::stop(&Paths::from_env()?)?;
            println!("{}", if stopped { "Stopped." } else { "Not running." });
            Ok(())
        }
        ["login"] => login::run(&Paths::from_env()?),
        ["tui"] => ui::run(&Paths::from_env()?),
        ["open"] => open_popup(),
        ["status"] => status(false),
        ["status", "--json"] => status(true),
        ["ctl", action @ ..] => ctl(&parse_action(action)?),
        ["library"] => browse(Request::GetLibrary { refresh: false }),
        ["library", "--refresh"] => browse(Request::GetLibrary { refresh: true }),
        ["tracks", uri] => browse(Request::GetTracks {
            uri: (*uri).to_owned(),
        }),
        ["tracks", uri, "--more"] => browse(Request::MoreTracks {
            uri: (*uri).to_owned(),
        }),
        ["search", words @ ..] if !words.is_empty() => browse(Request::Search {
            query: words.join(" "),
        }),
        _ => Err(format!("Unknown command: {}\n\n{USAGE}", args.join(" ")).into()),
    }
}

/// Opens the popup through Herdr. Works from a keybinding, with or without a pane.
fn open_popup() -> Result<()> {
    let herdr = std::env::var_os("HERDR_BIN_PATH").unwrap_or_else(|| "herdr".into());
    let status = std::process::Command::new(herdr)
        .args([
            "plugin",
            "pane",
            "open",
            "--plugin",
            "personal.spotify",
            "--entrypoint",
            "player",
        ])
        .status()?;
    if !status.success() {
        return Err("Could not open the Spotify popup".into());
    }
    Ok(())
}

fn status(json: bool) -> Result<()> {
    let mut connection = Connection::open(&Paths::from_env()?)?;
    let state = settled(&mut connection, None, SETTLE_WAIT)?;
    if json {
        let report = serde_json::json!({
            "pid": connection.daemon_pid,
            "build": connection.daemon_build,
            "state": state,
        });
        println!("{report}");
    } else {
        println!("{}", describe(&state));
    }
    Ok(())
}

fn ctl(request: &Request) -> Result<()> {
    let mut connection = Connection::open(&Paths::from_env()?)?;
    // Loading a context reports the context first and the track a moment later.
    let quiet = match request {
        Request::PlayContext { .. } | Request::PlayTracks { .. } => LOAD_WAIT,
        _ => SETTLE_WAIT,
    };
    let state = settled(&mut connection, Some(request), quiet)?;
    println!("{}", describe(&state));
    Ok(())
}

/// The state once the daemon has gone `quiet`, after sending `request` if there is one. A
/// daemon still connecting to Spotify is waited for, up to `EFFECT_WAIT` in all.
fn settled(
    connection: &mut Connection,
    request: Option<&Request>,
    quiet: Duration,
) -> Result<State> {
    let mut state = match connection.read()? {
        Some(Message::State(state)) => state,
        other => return Err(format!("Expected the player state, got {other:?}").into()),
    };
    let deadline = Instant::now() + EFFECT_WAIT;
    let mut wait = match request {
        Some(request) => {
            connection.send(request)?;
            EFFECT_WAIT
        }
        None => quiet,
    };
    loop {
        if state.status == Status::Connecting {
            wait = deadline.saturating_duration_since(Instant::now());
        }
        if wait.is_zero() {
            break;
        }
        match connection.read_within(wait)? {
            Some(Message::State(next)) => state = next,
            Some(Message::Notice { text }) => return Err(text.into()),
            Some(_) => {}
            None => break,
        }
        wait = quiet.min(deadline.saturating_duration_since(Instant::now()));
    }
    Ok(state)
}

/// How long a browsing command waits for Spotify's answer.
const BROWSE_WAIT: Duration = Duration::from_secs(30);

/// Sends a browsing request and prints the answer.
fn browse(request: Request) -> Result<()> {
    let request = match request {
        Request::GetTracks { uri } if uri == "liked" => Request::GetTracks { uri: LIKED.into() },
        Request::MoreTracks { uri } if uri == "liked" => Request::MoreTracks { uri: LIKED.into() },
        other => other,
    };
    let mut connection = Connection::open(&Paths::from_env()?)?;
    connection.send(&request)?;
    let deadline = Instant::now() + BROWSE_WAIT;
    let mut shown = false;
    // A stale library is sent first and the fresh one after it; print the last one to arrive.
    while let Some(message) =
        connection.read_within(deadline.saturating_duration_since(Instant::now()))?
    {
        // The daemon sends a second library when the first was stale or a refresh was asked for.
        let mut follow_up = false;
        let lines = match (&request, message) {
            (Request::GetLibrary { .. }, Message::Library(library)) => {
                if !library.signed_in {
                    return Err("Browsing needs its own sign-in. Run: herdr-spotify login".into());
                }
                let age = now_ms().saturating_sub(library.fetched_at_ms);
                let refresh = matches!(request, Request::GetLibrary { refresh: true });
                follow_up = !shown && (refresh || age >= LIBRARY_TTL_MS);
                let mut lines = vec![format!("Liked Songs · {} songs", library.liked_total)];
                lines.extend(library.playlists.iter().map(|playlist| {
                    format!(
                        "{} · {} songs · {}  {}",
                        playlist.name, playlist.total, playlist.owner, playlist.uri
                    )
                }));
                lines
            }
            (Request::GetTracks { .. } | Request::MoreTracks { .. }, Message::Tracks(list)) => {
                let mut lines: Vec<String> = list.tracks.iter().map(track_line).collect();
                if usize::try_from(list.total).unwrap_or(usize::MAX) > list.tracks.len() {
                    lines.push(format!("… {} of {} shown", list.tracks.len(), list.total));
                }
                lines
            }
            (Request::Search { .. }, Message::SearchResults(results)) => {
                let mut lines: Vec<String> = results.tracks.iter().map(track_line).collect();
                lines.extend(results.playlists.iter().map(|playlist| {
                    format!(
                        "playlist · {} · {}  {}",
                        playlist.name, playlist.owner, playlist.uri
                    )
                }));
                lines
            }
            (_, Message::Notice { text }) => return Err(text.into()),
            _ => continue,
        };
        if shown {
            println!("--- updated");
        }
        println!("{}", lines.join("\n"));
        shown = true;
        if !follow_up {
            break;
        }
    }
    if shown {
        Ok(())
    } else {
        Err("Spotify did not answer in time".into())
    }
}

fn track_line(track: &Track) -> String {
    format!(
        "{} — {} · {}  {}",
        track.name,
        track.artists.join(", "),
        clock(track.duration_ms),
        track.uri
    )
}

fn parse_action(action: &[&str]) -> Result<Request> {
    let number = |value: &str| {
        value
            .parse::<u32>()
            .map_err(|_| format!("Not a number: {value}"))
    };
    Ok(match action {
        ["play"] => Request::Play,
        ["pause"] => Request::Pause,
        ["toggle"] => Request::Toggle,
        ["next"] => Request::Next,
        ["previous"] => Request::Previous,
        ["seek", seconds] => Request::Seek {
            position_ms: number(seconds)?.saturating_mul(1000),
        },
        ["volume", percent] => match u8::try_from(number(percent)?) {
            Ok(percent) if percent <= 100 => Request::Volume { percent },
            _ => return Err("Volume is 0 to 100".into()),
        },
        ["shuffle", "on"] => Request::Shuffle { on: true },
        ["shuffle", "off"] => Request::Shuffle { on: false },
        ["repeat", "off"] => Request::Repeat { mode: Repeat::Off },
        ["repeat", "context"] => Request::Repeat {
            mode: Repeat::Context,
        },
        ["repeat", "track"] => Request::Repeat {
            mode: Repeat::Track,
        },
        ["play-context", uri] => Request::PlayContext {
            uri: (*uri).to_owned(),
            index: None,
            track_uri: None,
        },
        ["play-context", uri, track] if track.starts_with("spotify:track:") => {
            Request::PlayContext {
                uri: (*uri).to_owned(),
                index: None,
                track_uri: Some((*track).to_owned()),
            }
        }
        ["play-tracks", index, uris @ ..] if !uris.is_empty() => Request::PlayTracks {
            uris: uris.iter().map(|uri| (*uri).to_owned()).collect(),
            index: number(index)?,
        },
        ["play-context", uri, index] => Request::PlayContext {
            uri: (*uri).to_owned(),
            index: Some(number(index)?),
            track_uri: None,
        },
        _ => return Err(format!("Unknown action: {}\n\n{USAGE}", action.join(" ")).into()),
    })
}

/// One line for people: `playing · Hide Away — Daya · 0:15 / 3:11 · volume 50%`.
pub fn describe(state: &State) -> String {
    match state.status {
        Status::SignedOut => return "Not signed in. Run: herdr-spotify login".into(),
        Status::Connecting => return "Connecting to Spotify".into(),
        Status::Unavailable => {
            let problem = state.problem.as_deref().unwrap_or("unknown problem");
            return format!("Playback unavailable: {problem}");
        }
        Status::Ready => {}
    }
    let Some(track) = &state.track else {
        return format!("Ready · nothing playing · volume {}%", state.volume);
    };
    format!(
        "{} · {} — {} · {} / {} · volume {}%",
        if state.playing { "playing" } else { "paused" },
        track.name,
        track.artists.join(", "),
        clock(state.position_at(now_ms())),
        clock(track.duration_ms),
        state.volume,
    )
}

fn clock(milliseconds: u32) -> String {
    let seconds = milliseconds / 1000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Track;

    #[test]
    fn actions_become_requests() -> Result<()> {
        assert_eq!(
            parse_action(&["seek", "90"])?,
            Request::Seek {
                position_ms: 90_000
            }
        );
        assert_eq!(
            parse_action(&["repeat", "track"])?,
            Request::Repeat {
                mode: Repeat::Track
            }
        );
        assert_eq!(
            parse_action(&["play-context", "spotify:playlist:x", "4"])?,
            Request::PlayContext {
                uri: "spotify:playlist:x".into(),
                index: Some(4),
                track_uri: None
            }
        );
        assert!(parse_action(&["volume", "101"]).is_err());
        assert!(parse_action(&["seek", "soon"]).is_err());
        assert!(parse_action(&["dance"]).is_err());
        Ok(())
    }

    #[test]
    fn describes_each_status() {
        let mut state = State {
            status: Status::SignedOut,
            ..State::default()
        };
        assert_eq!(describe(&state), "Not signed in. Run: herdr-spotify login");
        state.status = Status::Unavailable;
        state.problem = Some("offline".into());
        assert_eq!(describe(&state), "Playback unavailable: offline");
        state.status = Status::Ready;
        state.volume = 50;
        assert_eq!(describe(&state), "Ready · nothing playing · volume 50%");
        state.track = Some(Track {
            name: "Hide Away".into(),
            artists: vec!["Daya".into()],
            duration_ms: 191_368,
            ..Track::default()
        });
        state.position_ms = 68_054;
        assert_eq!(
            describe(&state),
            "paused · Hide Away — Daya · 1:08 / 3:11 · volume 50%"
        );
    }
}
