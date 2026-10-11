//! The two screens reached from the library: a list's tracks, and search.

use super::library::songs;
use super::list::{Item, Row};
use super::{Open, Search, text};
use crate::protocol::{Playlist, State, Track, TrackList};

pub(super) fn track_rows(
    open: &Open,
    tracks: Option<&TrackList>,
    loading_more: bool,
    state: &State,
) -> Vec<Row> {
    let Some(list) = tracks else {
        return vec![Row::Label("Loading…".into())];
    };
    let label = if list.complete {
        songs(list.total)
    } else {
        format!("{} · {} loaded", songs(list.total), list.tracks.len())
    };
    let mut rows = vec![Row::Label(label)];
    if list.tracks.is_empty() && list.complete {
        rows.push(Row::Label(format!("{} has no songs yet.", open.title)));
    }
    rows.extend(list.tracks.iter().map(|track| track_row(track, state)));
    if loading_more {
        rows.push(Row::Label("Loading more…".into()));
    }
    rows
}

fn track_row(track: &Track, state: &State) -> Row {
    Row::Item(Item {
        name: track.name.clone(),
        secondary: track.artists.join(", "),
        detail: text::clock(track.duration_ms),
        current: state
            .track
            .as_ref()
            .is_some_and(|playing| playing.uri == track.uri),
    })
}

fn playlist_row(playlist: &Playlist, state: &State) -> Row {
    Row::Item(Item {
        name: playlist.name.clone(),
        secondary: playlist.owner.clone(),
        detail: songs(playlist.total),
        current: state.context_uri.as_deref() == Some(playlist.uri.as_str()),
    })
}

/// The query line. While it is being typed a block marks the cursor.
pub(super) fn query_line(search: &Search) -> String {
    if search.editing {
        format!("/ {}█", search.query)
    } else {
        format!("/ {}", search.query)
    }
}

pub(super) fn search_rows(search: &Search, state: &State) -> Vec<Row> {
    let Some(results) = &search.results else {
        let remark = if search.waiting {
            "Searching…"
        } else {
            "Type what you are looking for and press Enter."
        };
        return vec![Row::Label(remark.into())];
    };
    if results.tracks.is_empty() && results.playlists.is_empty() {
        return vec![Row::Label(format!(
            "Nothing found for “{}”.",
            results.query
        ))];
    }
    let mut rows = Vec::new();
    if !results.tracks.is_empty() {
        rows.push(Row::Label("SONGS".into()));
        rows.extend(results.tracks.iter().map(|track| track_row(track, state)));
    }
    if !results.playlists.is_empty() {
        if !rows.is_empty() {
            rows.push(Row::Blank);
        }
        rows.push(Row::Label("PLAYLISTS".into()));
        rows.extend(
            results
                .playlists
                .iter()
                .map(|playlist| playlist_row(playlist, state)),
        );
    }
    rows
}
