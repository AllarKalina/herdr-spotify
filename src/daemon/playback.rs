//! Turns requests into player commands, and remembers where playback is.
//!
//! librespot ignores play, pause and skip until this computer is the active device with
//! something loaded. Until then those requests are answered from the resume point: the first
//! press of play loads what was playing before and carries on from there.

use super::PlayerEnd;
use super::resume::{Resume, Start};
use super::state::{self, Change};
use crate::paths::Paths;
use crate::protocol::{Message, Repeat, Request, now_ms};
use librespot_connect::Spirc;
use librespot_core::Error;
use librespot_playback::player::PlayerEvent;
use std::time::{Duration, Instant};

/// How often the position is written while a track plays.
const SAVE_EVERY: Duration = Duration::from_secs(10);

pub struct Playback {
    /// True while librespot has something loaded on this computer.
    loaded: bool,
    /// The tracks being played when there is no playlist around them.
    uris: Vec<String>,
    saved: Option<Resume>,
    saved_at: Instant,
}

impl Playback {
    /// Shows the saved point as a paused track, so the popup has it before Spotify answers.
    pub fn restore(end: &PlayerEnd, paths: &Paths) -> Self {
        let saved = Resume::load(paths);
        if let Some(resume) = &saved {
            change(end, Change::Restore(resume.clone()));
        }
        let uris = saved
            .as_ref()
            .map(|resume| resume.uris.clone())
            .unwrap_or_default();
        Self {
            loaded: false,
            uris,
            saved,
            saved_at: Instant::now(),
        }
    }

    /// The volume a new session should start at: the one last used on this computer.
    pub fn volume(&self) -> Option<u8> {
        self.saved.as_ref().map(|resume| resume.volume)
    }

    /// A new session has nothing loaded, whatever the last one had.
    pub fn session_ended(&mut self) {
        self.loaded = false;
    }

    pub fn handle(&mut self, spirc: &Spirc, request: Request, end: &PlayerEnd) {
        let outcome = if self.loaded {
            self.command(spirc, request, end)
        } else {
            self.before_loading(spirc, request, end)
        };
        if let Err(error) = outcome {
            log::warn!("request failed: {error}");
            notice(end, format!("Spotify request failed: {error}"));
        }
    }

    /// Requests while librespot plays on this computer go straight to it.
    fn command(&mut self, spirc: &Spirc, request: Request, end: &PlayerEnd) -> Result<(), Error> {
        match request {
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
            other => self.start_requested(spirc, other, end),
        }
    }

    /// Requests before anything is loaded: play and skip resume, the rest only change what
    /// resuming will do.
    fn before_loading(
        &mut self,
        spirc: &Spirc,
        request: Request,
        end: &PlayerEnd,
    ) -> Result<(), Error> {
        let resume = || Resume::of(&end.state.borrow(), &self.uris, now_ms());
        match request {
            Request::Play | Request::Toggle | Request::Next | Request::Previous => {
                let Some(resume) = resume() else {
                    notice(
                        end,
                        "Nothing to resume yet. Choose something to play.".into(),
                    );
                    return Ok(());
                };
                self.load(spirc, resume.start(true), resume.volume)?;
                match request {
                    Request::Next => spirc.next(),
                    Request::Previous => spirc.prev(),
                    _ => Ok(()),
                }
            }
            Request::Pause => Ok(()),
            Request::Seek { position_ms } => {
                change(end, Change::Position { position_ms });
                Ok(())
            }
            Request::Volume { percent } => {
                change(end, Change::Volume(percent));
                Ok(())
            }
            Request::Shuffle { on } => {
                change(end, Change::Shuffle(on));
                Ok(())
            }
            Request::Repeat { mode } => {
                change(end, Change::Repeat(mode));
                Ok(())
            }
            other => self.start_requested(spirc, other, end),
        }
    }

    /// Starts what the request names, keeping the shuffle, repeat and volume in effect.
    fn start_requested(
        &mut self,
        spirc: &Spirc,
        request: Request,
        end: &PlayerEnd,
    ) -> Result<(), Error> {
        let (shuffle, repeat, volume) = {
            let state = end.state.borrow();
            (state.shuffle, state.repeat, state.volume)
        };
        let start = match request {
            Request::PlayContext {
                uri,
                index,
                track_uri,
            } => {
                self.uris.clear();
                change(end, Change::Context(Some(uri.clone())));
                Start {
                    context_uri: Some(uri),
                    track_uri,
                    index,
                    shuffle,
                    repeat,
                    play: true,
                    ..Start::default()
                }
            }
            Request::PlayTracks { uris, index } => {
                self.uris.clone_from(&uris);
                change(end, Change::Context(None));
                Start {
                    uris,
                    index: Some(index),
                    shuffle,
                    repeat,
                    play: true,
                    ..Start::default()
                }
            }
            // Retry only matters without a session; the rest are answered elsewhere.
            _ => return Ok(()),
        };
        self.load(spirc, start, volume)
    }

    fn load(&mut self, spirc: &Spirc, start: Start, volume: u8) -> Result<(), Error> {
        spirc.activate()?;
        if !self.loaded && volume > 0 {
            spirc.set_volume(state::volume_raw(volume))?;
        }
        spirc.load(start.into_request())?;
        self.loaded = true;
        Ok(())
    }

    /// Notes what a player event says about whether something is loaded. Returns false for
    /// an event that must not reach the state: before anything is loaded, librespot reports
    /// its own default shuffle, repeat and volume, which are not the ones in effect.
    pub fn observe(&mut self, event: &PlayerEvent) -> bool {
        match event {
            PlayerEvent::Loading { .. }
            | PlayerEvent::Playing { .. }
            | PlayerEvent::Paused { .. } => {
                self.loaded = true;
            }
            PlayerEvent::Stopped { .. } => self.loaded = false,
            PlayerEvent::ShuffleChanged { .. }
            | PlayerEvent::RepeatChanged { .. }
            | PlayerEvent::VolumeChanged { .. } => return self.loaded || self.saved.is_none(),
            _ => {}
        }
        true
    }

    /// Writes the resume point when it changed, and every few seconds while playing.
    pub fn save(&mut self, end: &PlayerEnd, paths: &Paths, force: bool) {
        let Some(current) = Resume::of(&end.state.borrow(), &self.uris, now_ms()) else {
            return;
        };
        let playing = end.state.borrow().playing;
        let due = match &self.saved {
            None => true,
            Some(saved) if saved.differs_beyond_position(&current) => true,
            // While paused the position only moves when sought; while playing it always does.
            Some(saved) if !playing => saved.position_ms != current.position_ms,
            Some(_) => self.saved_at.elapsed() >= SAVE_EVERY,
        };
        if due || (force && self.saved.as_ref() != Some(&current)) {
            current.save(paths);
            self.saved = Some(current);
            self.saved_at = Instant::now();
        }
    }
}

fn notice(end: &PlayerEnd, text: String) {
    let _ = end.events.send(Message::Notice { text });
}

fn change(end: &PlayerEnd, change: Change) {
    end.state
        .send_if_modified(|current| state::apply(current, change, now_ms()));
}
