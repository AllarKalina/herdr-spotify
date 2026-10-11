//! How player events change the state clients see. Pure, so it is tested without Spotify.

use crate::protocol::{Repeat, State, Status, Track};

/// Something the player reported.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Track(Track),
    Playing {
        position_ms: u32,
    },
    Paused {
        position_ms: u32,
    },
    /// The position moved without playback starting or stopping: a seek or a correction.
    Position {
        position_ms: u32,
    },
    Stopped,
    Volume(u8),
    Shuffle(bool),
    Repeat(Repeat),
    Context(Option<String>),
    Status(Status, Option<String>),
}

/// Applies `change`. Returns false when nothing a client can see changed, so no update is sent.
pub fn apply(state: &mut State, change: Change, now_ms: u64) -> bool {
    let before = state.clone();
    match change {
        Change::Track(track) => {
            if state.track.as_ref().map(|current| &current.uri) != Some(&track.uri) {
                state.position_ms = 0;
                state.position_at_ms = now_ms;
            }
            state.track = Some(track);
        }
        Change::Playing { position_ms } => set_position(state, true, position_ms, now_ms),
        Change::Paused { position_ms } => set_position(state, false, position_ms, now_ms),
        Change::Position { position_ms } => set_position(state, state.playing, position_ms, now_ms),
        Change::Stopped => set_position(state, false, 0, now_ms),
        Change::Volume(volume) => state.volume = volume.min(100),
        Change::Shuffle(shuffle) => state.shuffle = shuffle,
        Change::Repeat(repeat) => state.repeat = repeat,
        Change::Context(uri) => state.context_uri = uri,
        Change::Status(status, problem) => {
            state.status = status;
            state.problem = problem;
            if status != Status::Ready {
                let position = state.position_at(now_ms);
                set_position(state, false, position, now_ms);
            }
        }
    }
    *state != before
}

fn set_position(state: &mut State, playing: bool, position_ms: u32, now_ms: u64) {
    state.playing = playing;
    state.position_ms = position_ms;
    state.position_at_ms = now_ms;
}

/// Spotify volume, 0 to 65535, as a percentage.
pub fn volume_percent(raw: u16) -> u8 {
    u8::try_from((u32::from(raw) * 100 + 32_767) / 65_535).unwrap_or(100)
}

pub fn volume_raw(percent: u8) -> u16 {
    u16::try_from(u32::from(percent.min(100)) * 65_535 / 100).unwrap_or(u16::MAX)
}

pub fn repeat(context: bool, track: bool) -> Repeat {
    match (track, context) {
        (true, _) => Repeat::Track,
        (false, true) => Repeat::Context,
        (false, false) => Repeat::Off,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(uri: &str) -> Track {
        Track {
            uri: uri.into(),
            duration_ms: 200_000,
            ..Track::default()
        }
    }

    #[test]
    fn a_new_track_starts_at_zero_and_the_same_track_keeps_its_position() {
        let mut state = State::default();
        apply(&mut state, Change::Track(track("a")), 10);
        apply(
            &mut state,
            Change::Playing {
                position_ms: 30_000,
            },
            20,
        );
        assert!(!apply(&mut state, Change::Track(track("a")), 30));
        assert_eq!(state.position_ms, 30_000);
        assert!(apply(&mut state, Change::Track(track("b")), 40));
        assert_eq!((state.position_ms, state.position_at_ms), (0, 40));
    }

    #[test]
    fn a_seek_keeps_playing_and_a_pause_records_where_it_stopped() {
        let mut state = State::default();
        apply(&mut state, Change::Playing { position_ms: 0 }, 0);
        apply(
            &mut state,
            Change::Position {
                position_ms: 60_000,
            },
            5,
        );
        assert!(state.playing);
        apply(
            &mut state,
            Change::Paused {
                position_ms: 68_000,
            },
            9,
        );
        assert_eq!(
            (state.playing, state.position_ms, state.position_at_ms),
            (false, 68_000, 9)
        );
    }

    #[test]
    fn losing_the_session_freezes_playback_where_it_was() {
        let mut state = State {
            status: Status::Ready,
            ..State::default()
        };
        apply(&mut state, Change::Track(track("a")), 0);
        apply(&mut state, Change::Playing { position_ms: 1_000 }, 0);
        apply(
            &mut state,
            Change::Status(Status::Unavailable, Some("offline".into())),
            4_000,
        );
        assert_eq!((state.playing, state.position_ms), (false, 5_000));
        assert_eq!(state.problem.as_deref(), Some("offline"));
    }

    #[test]
    fn an_unchanged_value_is_not_an_update() {
        let mut state = State::default();
        assert!(apply(&mut state, Change::Volume(40), 0));
        assert!(!apply(&mut state, Change::Volume(40), 1));
        assert!(!apply(&mut state, Change::Shuffle(false), 1));
    }

    #[test]
    fn volume_converts_both_ways() {
        assert_eq!(volume_percent(0), 0);
        assert_eq!(volume_percent(32_767), 50);
        assert_eq!(volume_percent(u16::MAX), 100);
        assert_eq!(volume_raw(100), u16::MAX);
        assert_eq!(volume_percent(volume_raw(37)), 37);
        assert_eq!(repeat(true, true), Repeat::Track);
        assert_eq!(repeat(true, false), Repeat::Context);
    }
}
