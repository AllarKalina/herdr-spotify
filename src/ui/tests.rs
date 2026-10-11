//! Golden snapshots of every popup state, text plus styling, at three sizes; and what keys do.
//!
//! Snapshots live in `tests/snapshots/ui/`. Record intended changes with
//! `scripts/regress --update-snapshots` and review them through `git diff`.

mod keys;

use super::*;
use crate::protocol::{Library, Playlist, Repeat, SearchResults, Track, TrackList};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use library::Entry;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use std::fmt::Write as _;
use std::path::PathBuf;

const SIZES: [(u16, u16); 3] = [(100, 35), (60, 24), (40, 18)];
const NOW: u64 = 1_000_000;

fn ready() -> App {
    let mut app = App {
        link: Link::Up,
        ..App::default()
    };
    app.state.status = Status::Ready;
    app.state.user = Some("allar".into());
    app.state.volume = 50;
    app
}

fn playing() -> App {
    let mut app = ready();
    app.state.track = Some(Track {
        uri: "spotify:track:1".into(),
        name: "Hide Away".into(),
        artists: vec!["Daya".into()],
        album: "Sit Still, Look Pretty".into(),
        duration_ms: 191_000,
        cover_path: Some("/cover.jpg".into()),
    });
    app.state.playing = true;
    app.state.position_ms = 15_000;
    app.state.position_at_ms = NOW;
    app.state.context_uri = Some("spotify:user:allar:collection".into());
    // A diagonal gradient stands in for a real cover.
    let pixels = (0..10u8)
        .flat_map(|row| (0..10u8).map(move |column| [row * 25, column * 25, 120]))
        .collect();
    app.cover.pin("/cover.jpg", cover::Art::from_pixels(pixels));
    app
}

fn render(app: &mut App, (width, height): (u16, u16)) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|frame| draw(frame, app, NOW)).expect("draw");
    terminal.backend().buffer().clone()
}

/// The buffer as text, then every styled run: `row columns foreground background modifiers`.
fn describe(buffer: &Buffer) -> String {
    let area = buffer.area();
    let mut text = String::new();
    let mut styles = String::new();
    for row in 0..area.height {
        let mut line = String::new();
        let mut run: Option<(u16, String)> = None;
        for column in 0..=area.width {
            let style = (column < area.width).then(|| {
                let cell = &buffer[(column, row)];
                line.push_str(cell.symbol());
                style_name(cell.fg, cell.bg, cell.modifier)
            });
            let style = style.filter(|name| !name.is_empty());
            if run.as_ref().map(|(_, name)| name) != style.as_ref() {
                if let Some((start, name)) = run.take() {
                    let _ = writeln!(styles, "{row:>2} {start:>3}-{:<3} {name}", column - 1);
                }
                run = style.map(|name| (column, name));
            }
        }
        let _ = writeln!(text, "{}", line.trim_end());
    }
    format!("{text}\nstyles:\n{styles}")
}

fn style_name(fg: Color, bg: Color, modifier: Modifier) -> String {
    let mut parts = Vec::new();
    if fg != Color::Reset {
        parts.push(format!("fg:{fg}"));
    }
    if bg != Color::Reset {
        parts.push(format!("bg:{bg}"));
    }
    if !modifier.is_empty() {
        parts.push(format!("{modifier:?}"));
    }
    parts.join(" ")
}

/// Compares every size with its snapshot, or records them when asked to.
fn check(name: &str, app: &mut App) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/ui");
    let update = std::env::var_os("HERDR_SPOTIFY_UPDATE_SNAPSHOTS").is_some();
    let mut failures = Vec::new();
    for size in SIZES {
        let actual = describe(&render(app, size));
        let file = dir.join(format!("{name}@{}x{}.txt", size.0, size.1));
        if update {
            std::fs::create_dir_all(&dir).expect("snapshot dir");
            std::fs::write(&file, &actual).expect("write snapshot");
        } else if std::fs::read_to_string(&file).ok().as_deref() != Some(actual.as_str()) {
            let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-snapshots-actual");
            std::fs::create_dir_all(&out).expect("actual dir");
            std::fs::write(out.join(file.file_name().expect("name")), &actual).expect("write");
            failures.push(file.display().to_string());
        }
    }
    assert!(
        failures.is_empty(),
        "snapshots differ (actual output in target/ui-snapshots-actual/): {failures:#?}"
    );
}

fn playlist(id: &str, name: &str, total: u32) -> Playlist {
    Playlist {
        uri: format!("spotify:playlist:{id}"),
        name: name.into(),
        owner: "Allar Kalina".into(),
        total,
        snapshot_id: "s".into(),
    }
}

fn song(id: u32, name: &str, artist: &str) -> Track {
    Track {
        uri: format!("spotify:track:{id}"),
        name: name.into(),
        artists: vec![artist.into()],
        album: "Album".into(),
        duration_ms: 180_000 + id * 1000,
        cover_path: None,
    }
}

/// Playing, with a library of three playlists.
fn browsing() -> App {
    let mut app = playing();
    app.library = Some(Library {
        signed_in: true,
        playlists: vec![
            playlist("p1", "Focus", 110),
            playlist(
                "p2",
                "A Playlist With A Name Much Longer Than Any Popup Is Wide, Probably",
                1,
            ),
            playlist("p3", "Running", 56),
        ],
        liked_total: 1085,
        fetched_at_ms: NOW,
    });
    app
}

fn focus() -> Entry {
    Entry {
        name: "Focus".into(),
        owner: "Allar Kalina".into(),
        detail: String::new(),
        play_uri: "spotify:playlist:p1".into(),
        tracks_uri: "spotify:playlist:p1".into(),
    }
}

/// The Focus playlist open with `count` of 110 tracks loaded.
fn in_playlist(count: u32) -> App {
    let mut app = browsing();
    app.open(&focus(), false);
    let tracks = (1..=count)
        .map(|id| song(id, &format!("Song {id}"), "Some Artist"))
        .collect();
    app.apply(Message::Tracks(TrackList {
        uri: "spotify:playlist:p1".into(),
        tracks,
        total: 110,
        complete: count >= 110,
    }));
    app
}

fn searched() -> App {
    let mut app = browsing();
    app.screen = Screen::Search;
    app.search.query = "daft".into();
    app.apply(Message::SearchResults(SearchResults {
        query: "daft".into(),
        tracks: vec![
            song(1, "Hide Away", "Daya"),
            song(7, "One More Time", "Daft Punk"),
        ],
        playlists: vec![playlist("p9", "Daft Punk Radio", 50)],
    }));
    app
}

#[test]
fn snapshot_library() {
    check("library", &mut browsing());

    let mut app = playing();
    app.state.playing = false;
    app.state.shuffle = true;
    app.state.repeat = Repeat::Track;
    let track = app.state.track.as_mut().expect("track");
    track.cover_path = None;
    track.name =
        "A Remarkably Long Track Title That Cannot Possibly Fit On One Line (Live, Remastered)"
            .into();
    check("library-loading", &mut app);

    app.library = Some(Library::default());
    check("library-browsing-signed-out", &mut app);
}

#[test]
fn snapshot_nothing_playing() {
    let mut app = ready();
    app.library = browsing().library;
    check("nothing-playing", &mut app);
}

#[test]
fn snapshot_tracks() {
    let mut app = in_playlist(50);
    app.tracks_cursor.move_by(1, 50);
    app.state.track.as_mut().expect("track").uri = "spotify:track:4".into();
    check("tracks", &mut app);

    app.loading_more = true;
    app.tracks_cursor.move_by(48, 50);
    check("tracks-loading-more", &mut app);

    let mut waiting = browsing();
    waiting.open(&focus(), false);
    check("tracks-loading", &mut waiting);
}

#[test]
fn snapshot_search() {
    let mut app = browsing();
    app.screen = Screen::Search;
    app.search.editing = true;
    check("search-empty-query", &mut app);
    app.search.query = "daft".into();
    check("search-typing", &mut app);
    app.search.editing = false;
    app.search.waiting = true;
    check("search-waiting", &mut app);

    let mut found = searched();
    found.search.cursor.move_by(2, 3);
    check("search-results", &mut found);

    found.apply(Message::SearchResults(SearchResults {
        query: "daft".into(),
        ..SearchResults::default()
    }));
    check("search-nothing-found", &mut found);
}

#[test]
fn snapshot_states_without_a_player() {
    check("starting", &mut App::default());

    let mut app = App {
        link: Link::Up,
        ..App::default()
    };
    check("connecting", &mut app);
    app.state.status = Status::SignedOut;
    check("signed-out", &mut app);
    app.state.status = Status::Unavailable;
    app.state.problem = Some("Cannot reach Spotify: the network is down".into());
    check("unavailable", &mut app);

    let mut app = App {
        link: Link::Down("It stopped.".into()),
        ..App::default()
    };
    check("stopped", &mut app);
}

#[test]
fn snapshot_notice_above_the_shortcuts() {
    let mut app = browsing();
    app.notify("Spotify is rate limiting requests; trying again in 45s".into());
    check("notice", &mut app);
}

#[test]
fn every_screen_draws_at_any_size_without_panicking() {
    let mut apps = vec![
        browsing(),
        in_playlist(50),
        searched(),
        ready(),
        App::default(),
    ];
    apps.push(App {
        link: Link::Down("gone".into()),
        ..App::default()
    });
    for app in &mut apps {
        for width in [0, 1, 4, 11, 20, 56, 57, 64, 200] {
            for height in [0, 1, 3, 5, 9, 14, 60] {
                render(app, (width, height));
            }
        }
    }
}

#[test]
fn the_selection_stays_in_view_while_scrolling_down_and_back_up() {
    let mut app = in_playlist(50);
    let size = (60, 24);
    let visible = |app: &mut App, name: &str| describe(&render(app, size)).contains(name);
    assert!(visible(&mut app, "Song 1 "));
    app.tracks_cursor.move_by(40, 50);
    assert!(visible(&mut app, "Song 41 "));
    assert!(!visible(&mut app, "Song 1 "));
    app.tracks_cursor.move_by(-40, 50);
    assert!(visible(&mut app, "Song 1 "));
    assert!(
        visible(&mut app, "110 songs"),
        "the label returns with the first row"
    );
}
