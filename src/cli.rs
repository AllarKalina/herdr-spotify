//! The command line: run or stop the daemon, sign in, and drive playback from a shell.

use crate::client::{self, Connection};
use crate::paths::Paths;
use crate::protocol::{Message, Repeat, Request, State, Status, now_ms};
use crate::{Result, daemon, login};
use std::time::{Duration, Instant};

const USAGE: &str = "\
Usage: herdr-spotify <command>

  login                       Sign in to Spotify for audio playback on this computer
  status [--json]             Show what is playing (starts the daemon if needed)
  ctl <action>                Control playback:
        play | pause | toggle | next | previous
        seek <seconds> | volume <0-100> | shuffle <on|off> | repeat <off|context|track>
        play-context <spotify-uri> [track-index]
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
        ["status"] => status(false),
        ["status", "--json"] => status(true),
        ["ctl", action @ ..] => ctl(&parse_action(action)?),
        _ => Err(format!("Unknown command: {}\n\n{USAGE}", args.join(" ")).into()),
    }
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
        Request::PlayContext { .. } => LOAD_WAIT,
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
            Some(Message::Hello { .. }) => {}
            None => break,
        }
        wait = quiet.min(deadline.saturating_duration_since(Instant::now()));
    }
    Ok(state)
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
        },
        ["play-context", uri, index] => Request::PlayContext {
            uri: (*uri).to_owned(),
            index: Some(number(index)?),
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
                index: Some(4)
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
