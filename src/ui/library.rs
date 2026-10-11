//! The library screen: Liked Songs, then the playlists.

use super::list::{Item, Row};
use crate::protocol::{LIKED, Library, State};

/// Something in the library that can be played whole, or opened to see its tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub owner: String,
    pub detail: String,
    /// The Spotify context that plays it.
    pub play_uri: String,
    /// What to ask the daemon for to list its tracks.
    pub tracks_uri: String,
}

/// Liked Songs needs only the audio session; playlists appear once browsing is signed in.
pub fn entries(state: &State, library: Option<&Library>) -> Vec<Entry> {
    let mut entries = Vec::new();
    if let Some(user) = &state.user {
        let count = library
            .filter(|library| library.signed_in)
            .map(|library| library.liked_total);
        entries.push(Entry {
            name: "♥ Liked Songs".into(),
            owner: String::new(),
            detail: count.map(songs).unwrap_or_default(),
            play_uri: format!("spotify:user:{user}:collection"),
            tracks_uri: LIKED.into(),
        });
    }
    for playlist in library.into_iter().flat_map(|library| &library.playlists) {
        entries.push(Entry {
            name: playlist.name.clone(),
            owner: playlist.owner.clone(),
            detail: songs(playlist.total),
            play_uri: playlist.uri.clone(),
            tracks_uri: playlist.uri.clone(),
        });
    }
    entries
}

pub(super) fn songs(count: u32) -> String {
    if count == 1 {
        "1 song".into()
    } else {
        format!("{count} songs")
    }
}

pub(super) fn rows(state: &State, library: Option<&Library>) -> Vec<Row> {
    let mut rows = vec![Row::Label("LIBRARY".into())];
    rows.extend(entries(state, library).into_iter().map(|entry| {
        Row::Item(Item {
            current: state.context_uri.as_deref() == Some(entry.play_uri.as_str()),
            name: entry.name,
            secondary: entry.owner,
            detail: entry.detail,
        })
    }));
    match library {
        None => rows.push(Row::Label("Loading playlists…".into())),
        Some(library) if !library.signed_in => {
            rows.push(Row::Blank);
            rows.push(Row::Label(
                "Playlists and search need a second sign-in.".into(),
            ));
            rows.push(Row::Label("In a terminal: herdr-spotify login".into()));
        }
        Some(_) => {}
    }
    rows
}
