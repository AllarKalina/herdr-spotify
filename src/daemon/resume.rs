//! Where playback was, kept on disk so a new daemon can carry on from it.

use crate::paths::Paths;
use crate::protocol::{Repeat, State, Track};
use librespot_connect::{
    LoadContextOptions, LoadRequest, LoadRequestOptions, Options, PlayingTrack,
};
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resume {
    /// The playlist or collection being played, when there was one.
    #[serde(default)]
    pub context_uri: Option<String>,
    /// The tracks being played when there was no playlist around them, such as search results.
    #[serde(default)]
    pub uris: Vec<String>,
    pub track: Track,
    pub position_ms: u32,
    pub volume: u8,
    pub shuffle: bool,
    pub repeat: Repeat,
}

impl Resume {
    /// The point to resume from, if anything is loaded.
    pub fn of(state: &State, uris: &[String], now_ms: u64) -> Option<Self> {
        Some(Self {
            context_uri: state.context_uri.clone(),
            uris: if state.context_uri.is_some() {
                Vec::new()
            } else {
                uris.to_vec()
            },
            track: state.track.clone()?,
            position_ms: state.position_at(now_ms),
            volume: state.volume,
            shuffle: state.shuffle,
            repeat: state.repeat,
        })
    }

    /// Reads the saved point. Anything unreadable counts as nothing to resume.
    pub fn load(paths: &Paths) -> Option<Self> {
        serde_json::from_str(&fs::read_to_string(paths.resume()).ok()?).ok()
    }

    /// Writes the point in one step. A failure is logged; playback carries on regardless.
    pub fn save(&self, paths: &Paths) {
        let outcome = (|| -> crate::Result<()> {
            let path = paths.resume();
            let partial = path.with_extension("partial");
            fs::write(&partial, serde_json::to_vec(self)?)?;
            fs::rename(&partial, &path)?;
            Ok(())
        })();
        if let Err(error) = outcome {
            log::warn!("resume point not saved: {error}");
        }
    }

    /// True when the two points differ in anything but how far the track has played.
    pub fn differs_beyond_position(&self, other: &Self) -> bool {
        let same_place = Self {
            position_ms: other.position_ms,
            ..self.clone()
        };
        same_place != *other
    }

    /// How to load this point back into the player.
    pub fn start(&self, play: bool) -> Start {
        let (context_uri, uris, index) = match (&self.context_uri, self.uris.as_slice()) {
            (Some(context), _) => (Some(context.clone()), Vec::new(), None),
            // Playback came from elsewhere and its surroundings are unknown: the track alone.
            (None, []) => (None, vec![self.track.uri.clone()], Some(0)),
            (None, uris) => {
                let index = uris
                    .iter()
                    .position(|uri| *uri == self.track.uri)
                    .unwrap_or(0);
                (None, uris.to_vec(), u32::try_from(index).ok())
            }
        };
        Start {
            track_uri: context_uri.is_some().then(|| self.track.uri.clone()),
            context_uri,
            uris,
            index,
            position_ms: self.position_ms,
            shuffle: self.shuffle,
            repeat: self.repeat,
            play,
        }
    }
}

/// Something to load into the player: a context or a list of tracks, and where in it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Start {
    pub context_uri: Option<String>,
    pub uris: Vec<String>,
    /// Start at this track of the context.
    pub track_uri: Option<String>,
    /// Start at this position of the context or of `uris`.
    pub index: Option<u32>,
    pub position_ms: u32,
    pub shuffle: bool,
    pub repeat: Repeat,
    pub play: bool,
}

impl Start {
    pub fn into_request(self) -> LoadRequest {
        let playing_track = self
            .track_uri
            .map(PlayingTrack::Uri)
            .or(self.index.map(PlayingTrack::Index));
        let options = LoadRequestOptions {
            start_playing: self.play,
            seek_to: self.position_ms,
            playing_track,
            context_options: Some(LoadContextOptions::Options(Options {
                shuffle: self.shuffle,
                repeat: self.repeat == Repeat::Context,
                repeat_track: self.repeat == Repeat::Track,
            })),
        };
        match self.context_uri {
            Some(context) => LoadRequest::from_context_uri(context, options),
            None => LoadRequest::from_tracks(self.uris, options),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn state() -> State {
        State {
            track: Some(Track {
                uri: "spotify:track:b".into(),
                duration_ms: 200_000,
                ..Track::default()
            }),
            playing: true,
            position_ms: 10_000,
            position_at_ms: 1_000,
            volume: 35,
            shuffle: true,
            repeat: Repeat::Context,
            context_uri: Some("spotify:playlist:p".into()),
            ..State::default()
        }
    }

    #[test]
    fn the_point_records_where_playback_is_now_not_where_it_was_reported() {
        let resume = Resume::of(&state(), &[], 6_000).expect("resume");
        assert_eq!(resume.position_ms, 15_000);
        assert_eq!(
            (resume.volume, resume.shuffle, resume.repeat),
            (35, true, Repeat::Context)
        );
        assert_eq!(
            Resume::of(&State::default(), &[], 0),
            None,
            "nothing loaded, nothing to resume"
        );
    }

    #[test]
    fn a_playlist_resumes_at_its_track_and_position_with_the_same_settings() {
        let start = Resume::of(&state(), &["ignored".into()], 1_000)
            .expect("resume")
            .start(true);
        assert_eq!(start.context_uri.as_deref(), Some("spotify:playlist:p"));
        assert_eq!(start.track_uri.as_deref(), Some("spotify:track:b"));
        assert!(start.uris.is_empty());
        assert_eq!(
            (start.position_ms, start.shuffle, start.repeat, start.play),
            (10_000, true, Repeat::Context, true)
        );
    }

    #[test]
    fn loose_tracks_resume_at_the_one_that_was_playing() {
        let mut state = state();
        state.context_uri = None;
        let uris: Vec<String> = ["a", "b", "c"]
            .iter()
            .map(|id| format!("spotify:track:{id}"))
            .collect();
        let start = Resume::of(&state, &uris, 1_000)
            .expect("resume")
            .start(false);
        assert_eq!(
            (start.context_uri, start.uris.len(), start.index, start.play),
            (None, 3, Some(1), false)
        );

        // With nothing known about its surroundings, the track plays on its own.
        let alone = Resume::of(&state, &[], 1_000).expect("resume").start(true);
        assert_eq!(
            (alone.uris, alone.index),
            (vec!["spotify:track:b".to_owned()], Some(0))
        );
    }

    #[test]
    fn the_point_survives_the_disk_and_a_damaged_file_is_nothing_to_resume() {
        let dir = PathBuf::from(format!("/tmp/hs-resume-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let paths = Paths::at(dir.clone()).expect("paths");
        paths.prepare().expect("dir");
        assert_eq!(Resume::load(&paths), None);

        let resume = Resume::of(&state(), &[], 1_000).expect("resume");
        resume.save(&paths);
        assert_eq!(Resume::load(&paths), Some(resume.clone()));
        assert!(!resume.differs_beyond_position(&Resume {
            position_ms: 99,
            ..resume.clone()
        }));
        let louder = Resume {
            volume: 1,
            ..resume.clone()
        };
        assert!(resume.differs_beyond_position(&louder));

        fs::write(paths.resume(), "{not json").expect("damage");
        assert_eq!(Resume::load(&paths), None);
        fs::remove_dir_all(&dir).expect("clean");
    }
}
