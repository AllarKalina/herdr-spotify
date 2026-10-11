//! The wire format between the daemon and its clients: one JSON object per line.
//!
//! The daemon opens every connection with `Hello` and the current `State`, then pushes a new
//! `State` whenever anything changes. Clients send `Request`s and never poll.

use serde::{Deserialize, Serialize};

/// Bumped on any incompatible change to the messages below.
pub const PROTOCOL: u32 = 1;

/// Daemon to client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    Hello {
        protocol: u32,
        /// Identifies the daemon's executable; see `client::build_stamp`.
        build: String,
        pid: u32,
    },
    State(State),
    Notice {
        text: String,
    },
}

/// Client to daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
    Seek {
        position_ms: u32,
    },
    Volume {
        percent: u8,
    },
    Shuffle {
        on: bool,
    },
    Repeat {
        mode: Repeat,
    },
    /// Start a playlist, album or the liked-songs collection, optionally at a given track.
    PlayContext {
        uri: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<u32>,
    },
    /// Stop the daemon.
    Shutdown,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub status: Status,
    /// Why playback is unavailable, when `status` says it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<Track>,
    pub playing: bool,
    /// Position at `position_at_ms`. While playing, the current position is this plus the time
    /// elapsed since then.
    pub position_ms: u32,
    /// Unix time in milliseconds when `position_ms` was true.
    pub position_at_ms: u64,
    pub volume: u8,
    pub shuffle: bool,
    pub repeat: Repeat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_uri: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// No audio sign-in is stored; `herdr-spotify login` fixes it.
    SignedOut,
    #[default]
    Connecting,
    Ready,
    /// Signed in, but Spotify cannot be reached or refused the session.
    Unavailable,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Repeat {
    #[default]
    Off,
    Context,
    Track,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub uri: String,
    pub name: String,
    pub artists: Vec<String>,
    pub album: String,
    pub duration_ms: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_url: Option<String>,
}

impl State {
    /// The playback position at `now_ms`, extrapolated while playing.
    pub fn position_at(&self, now_ms: u64) -> u32 {
        if !self.playing {
            return self.position_ms;
        }
        let elapsed = now_ms.saturating_sub(self.position_at_ms);
        let position = u64::from(self.position_ms).saturating_add(elapsed);
        let limit = self
            .track
            .as_ref()
            .map_or(u32::MAX, |track| track.duration_ms);
        u32::try_from(position).unwrap_or(u32::MAX).min(limit)
    }
}

/// Unix time in milliseconds.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// One message as a line, newline included.
pub fn encode<T: Serialize>(message: &T) -> crate::Result<String> {
    let mut line = serde_json::to_string(message)?;
    line.push('\n');
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_use_a_type_tag() -> crate::Result<()> {
        assert_eq!(encode(&Request::Play)?, "{\"type\":\"play\"}\n");
        assert_eq!(
            encode(&Request::PlayContext {
                uri: "spotify:playlist:x".into(),
                index: Some(3)
            })?,
            "{\"type\":\"play_context\",\"uri\":\"spotify:playlist:x\",\"index\":3}\n"
        );
        let parsed: Request = serde_json::from_str(r#"{"type":"play_context","uri":"u"}"#)?;
        assert_eq!(
            parsed,
            Request::PlayContext {
                uri: "u".into(),
                index: None
            }
        );
        Ok(())
    }

    #[test]
    fn state_survives_a_round_trip() -> crate::Result<()> {
        let state = State {
            status: Status::Ready,
            track: Some(Track {
                name: "Hide Away".into(),
                duration_ms: 191_368,
                ..Track::default()
            }),
            playing: true,
            position_ms: 15_000,
            repeat: Repeat::Track,
            ..State::default()
        };
        let line = encode(&Message::State(state.clone()))?;
        assert!(line.starts_with("{\"type\":\"state\",\"status\":\"ready\""));
        assert_eq!(
            serde_json::from_str::<Message>(&line)?,
            Message::State(state)
        );
        Ok(())
    }

    #[test]
    fn position_advances_only_while_playing_and_stops_at_the_end() {
        let mut state = State {
            track: Some(Track {
                duration_ms: 10_000,
                ..Track::default()
            }),
            playing: true,
            position_ms: 4_000,
            position_at_ms: 1_000,
            ..State::default()
        };
        assert_eq!(state.position_at(3_000), 6_000);
        assert_eq!(state.position_at(60_000), 10_000);
        state.playing = false;
        assert_eq!(state.position_at(3_000), 4_000);
    }
}
