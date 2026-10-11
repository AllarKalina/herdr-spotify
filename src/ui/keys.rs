//! What each key does, per screen. Nothing here draws or talks to the daemon.

use super::library::{self, Entry};
use super::{Action, App, Link, Screen};
use crate::protocol::{Repeat, Request, State, Status};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const SEEK_STEP_MS: u32 = 10_000;
const VOLUME_STEP: u8 = 5;
const PAGE_ROWS: isize = 10;
/// The next page is asked for when the selection comes this close to the end of what is loaded.
const LOAD_AHEAD: usize = 10;

pub fn handle_key(app: &mut App, key: KeyEvent, now_ms: u64) -> Action {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return Action::Quit;
    }
    let typing = app.ready() && app.screen == Screen::Search && app.search.editing;
    if typing {
        return type_query(app, key.code);
    }
    if key.code == KeyCode::Char('q') {
        return Action::Quit;
    }
    if app.link != Link::Up {
        return match key.code {
            KeyCode::Esc => Action::Quit,
            KeyCode::Enter if app.link != Link::Starting => Action::Reconnect,
            _ => Action::None,
        };
    }
    if app.state.status != Status::Ready {
        return match key.code {
            KeyCode::Esc => Action::Quit,
            KeyCode::Enter if app.state.status != Status::Connecting => {
                Action::Send(Request::Retry)
            }
            _ => Action::None,
        };
    }
    if let Some(action) = transport(&app.state, key.code, now_ms) {
        return action;
    }
    if key.code == KeyCode::Char('/') {
        app.screen = Screen::Search;
        app.search.editing = true;
        return Action::None;
    }
    match app.screen.clone() {
        Screen::Library => library_key(app, key.code),
        Screen::Tracks(open) => tracks_key(app, &open, key.code),
        Screen::Search => results_key(app, key.code),
    }
}

/// Keys that mean the same on every screen.
fn transport(state: &State, code: KeyCode, now_ms: u64) -> Option<Action> {
    let request = match code {
        // With nothing loaded there is nothing to resume; the screen decides what Space starts.
        KeyCode::Char(' ') if state.track.is_some() => Request::Toggle,
        KeyCode::Char('n') => Request::Next,
        KeyCode::Char('p') => Request::Previous,
        KeyCode::Char('.') => seek(state, now_ms, true)?,
        KeyCode::Char(',') => seek(state, now_ms, false)?,
        KeyCode::Char('+' | '=') => Request::Volume {
            percent: state.volume.saturating_add(VOLUME_STEP).min(100),
        },
        KeyCode::Char('-') => Request::Volume {
            percent: state.volume.saturating_sub(VOLUME_STEP),
        },
        KeyCode::Char('s') => Request::Shuffle { on: !state.shuffle },
        KeyCode::Char('r') => Request::Repeat {
            mode: match state.repeat {
                Repeat::Off => Repeat::Context,
                Repeat::Context => Repeat::Track,
                Repeat::Track => Repeat::Off,
            },
        },
        _ => return None,
    };
    Some(Action::Send(request))
}

fn seek(state: &State, now_ms: u64, forward: bool) -> Option<Request> {
    let track = state.track.as_ref()?;
    let position = state.position_at(now_ms);
    let target = if forward {
        position.saturating_add(SEEK_STEP_MS).min(track.duration_ms)
    } else {
        position.saturating_sub(SEEK_STEP_MS)
    };
    Some(Request::Seek {
        position_ms: target,
    })
}

/// How far a movement key moves the selection, if it is one.
fn movement(code: KeyCode) -> Option<isize> {
    match code {
        KeyCode::Down | KeyCode::Char('j') => Some(1),
        KeyCode::Up | KeyCode::Char('k') => Some(-1),
        KeyCode::PageDown => Some(PAGE_ROWS),
        KeyCode::PageUp => Some(-PAGE_ROWS),
        _ => None,
    }
}

fn play(entry: &Entry) -> Action {
    Action::Send(Request::PlayContext {
        uri: entry.play_uri.clone(),
        index: None,
        track_uri: None,
    })
}

fn library_key(app: &mut App, code: KeyCode) -> Action {
    let entries = library::entries(&app.state, app.library.as_ref());
    if let Some(delta) = movement(code) {
        app.library_cursor.move_by(delta, entries.len());
        return Action::None;
    }
    let selected = entries.get(app.library_cursor.selected);
    match (code, selected) {
        (KeyCode::Esc, _) => Action::Quit,
        (KeyCode::Char('R'), _) => Action::Send(Request::GetLibrary { refresh: true }),
        (KeyCode::Enter | KeyCode::Char(' '), Some(entry)) => play(entry),
        (KeyCode::Right | KeyCode::Char('l'), Some(entry)) => app.open(entry, false),
        _ => Action::None,
    }
}

fn tracks_key(app: &mut App, open: &super::Open, code: KeyCode) -> Action {
    let loaded = app.tracks.as_ref().map_or(0, |list| list.tracks.len());
    if let Some(delta) = movement(code) {
        app.tracks_cursor.move_by(delta, loaded);
        let near_end = app.tracks_cursor.selected + LOAD_AHEAD >= loaded;
        let more = app.tracks.as_ref().is_some_and(|list| !list.complete);
        if delta > 0 && near_end && more && !app.loading_more {
            app.loading_more = true;
            return Action::Send(Request::MoreTracks {
                uri: open.tracks_uri.clone(),
            });
        }
        return Action::None;
    }
    match code {
        KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => {
            app.screen = if open.from_search {
                Screen::Search
            } else {
                Screen::Library
            };
            app.tracks = None;
            Action::None
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            let track = app
                .tracks
                .as_ref()
                .and_then(|list| list.tracks.get(app.tracks_cursor.selected));
            match track {
                Some(track) => Action::Send(Request::PlayContext {
                    uri: open.play_uri.clone(),
                    index: None,
                    track_uri: Some(track.uri.clone()),
                }),
                None => Action::None,
            }
        }
        _ => Action::None,
    }
}

/// While the query is typed, every key types; only Enter, Esc and Backspace act.
fn type_query(app: &mut App, code: KeyCode) -> Action {
    match code {
        KeyCode::Char(character) => app.search.query.push(character),
        KeyCode::Backspace => {
            app.search.query.pop();
        }
        KeyCode::Esc => {
            app.search.editing = false;
            // With results on screen Esc returns to them; with none it leaves search.
            if app.search.results.is_none() {
                app.screen = Screen::Library;
            }
        }
        KeyCode::Enter => {
            let query = app.search.query.trim().to_owned();
            if !query.is_empty() {
                app.search.editing = false;
                app.search.waiting = true;
                app.search.results = None;
                return Action::Send(Request::Search { query });
            }
        }
        _ => {}
    }
    Action::None
}

fn results_key(app: &mut App, code: KeyCode) -> Action {
    let (tracks, playlists) = match &app.search.results {
        Some(results) => (results.tracks.len(), results.playlists.len()),
        None => (0, 0),
    };
    if let Some(delta) = movement(code) {
        app.search.cursor.move_by(delta, tracks + playlists);
        return Action::None;
    }
    let selected = app.search.cursor.selected;
    let results = app.search.results.as_ref();
    let playlist = results
        .and_then(|results| results.playlists.get(selected.checked_sub(tracks)?))
        .map(|playlist| Entry {
            name: playlist.name.clone(),
            owner: playlist.owner.clone(),
            detail: String::new(),
            play_uri: playlist.uri.clone(),
            tracks_uri: playlist.uri.clone(),
        });
    match code {
        KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => {
            app.screen = Screen::Library;
            Action::None
        }
        KeyCode::Enter | KeyCode::Char(' ') => match (results, playlist) {
            (_, Some(playlist)) => play(&playlist),
            // A found song plays with the other found songs after it.
            (Some(results), None) if selected < tracks => Action::Send(Request::PlayTracks {
                uris: results
                    .tracks
                    .iter()
                    .map(|track| track.uri.clone())
                    .collect(),
                index: u32::try_from(selected).unwrap_or(0),
            }),
            _ => Action::None,
        },
        KeyCode::Right | KeyCode::Char('l') => match playlist {
            Some(playlist) => app.open(&playlist, true),
            None => Action::None,
        },
        _ => Action::None,
    }
}
