//! What keys do on each screen: the requests they send and where they lead.

use super::*;

fn press(app: &mut App, code: KeyCode) -> Action {
    handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), NOW)
}

fn send(request: Request) -> Action {
    Action::Send(request)
}

fn type_text(app: &mut App, text: &str) {
    for character in text.chars() {
        assert_eq!(press(app, KeyCode::Char(character)), Action::None);
    }
}

fn play_context(uri: &str, track_uri: Option<&str>) -> Action {
    send(Request::PlayContext {
        uri: uri.into(),
        index: None,
        track_uri: track_uri.map(str::to_owned),
    })
}

#[test]
fn in_the_library_enter_plays_a_row_and_right_opens_it() {
    let mut app = browsing();
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        play_context("spotify:user:allar:collection", None)
    );
    press(&mut app, KeyCode::Down);
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        play_context("spotify:playlist:p1", None)
    );

    assert_eq!(
        press(&mut app, KeyCode::Right),
        send(Request::GetTracks {
            uri: "spotify:playlist:p1".into()
        })
    );
    assert!(
        matches!(&app.screen, Screen::Tracks(open) if open.title == "Focus" && !open.from_search)
    );
    assert!(app.tracks.is_none());

    let mut liked = browsing();
    assert_eq!(
        press(&mut liked, KeyCode::Char('l')),
        send(Request::GetTracks {
            uri: "spotify:collection".into()
        })
    );
    assert!(matches!(&liked.screen, Screen::Tracks(open)
        if open.title == "Liked Songs" && open.play_uri == "spotify:user:allar:collection"));
}

#[test]
fn the_library_selection_stops_at_both_ends_and_refresh_asks_spotify() {
    let mut app = browsing();
    press(&mut app, KeyCode::Up);
    assert_eq!(app.library_cursor.selected, 0);
    for _ in 0..9 {
        press(&mut app, KeyCode::Char('j'));
    }
    assert_eq!(app.library_cursor.selected, 3);
    assert_eq!(
        press(&mut app, KeyCode::Char('R')),
        send(Request::GetLibrary { refresh: true })
    );
    assert_eq!(press(&mut app, KeyCode::Esc), Action::Quit);
}

#[test]
fn in_a_list_enter_plays_that_track_in_its_playlist_and_left_goes_back() {
    let mut app = in_playlist(50);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        play_context("spotify:playlist:p1", Some("spotify:track:3"))
    );

    assert_eq!(press(&mut app, KeyCode::Left), Action::None);
    assert_eq!(app.screen, Screen::Library);
    assert_eq!(
        app.library_cursor.selected, 0,
        "the library keeps its own selection"
    );

    let mut app = in_playlist(50);
    assert_eq!(
        press(&mut app, KeyCode::Esc),
        Action::None,
        "Esc goes back, it does not close"
    );
    assert_eq!(app.screen, Screen::Library);
}

#[test]
fn the_next_page_is_asked_for_once_when_the_selection_nears_the_end() {
    let mut app = in_playlist(50);
    let more = send(Request::MoreTracks {
        uri: "spotify:playlist:p1".into(),
    });
    for _ in 0..39 {
        assert_eq!(
            press(&mut app, KeyCode::Down),
            Action::None,
            "far from the end, nothing is asked"
        );
    }
    assert_eq!(press(&mut app, KeyCode::Down), more);
    assert!(app.loading_more);
    assert_eq!(
        press(&mut app, KeyCode::Down),
        Action::None,
        "one request at a time"
    );
    assert_eq!(
        press(&mut app, KeyCode::Up),
        Action::None,
        "moving away never asks"
    );

    // The page arrives: the selection is kept and further pages can be asked for.
    let selected = app.tracks_cursor.selected;
    let tracks = (1..=100).map(|id| song(id, "Song", "Artist")).collect();
    app.apply(Message::Tracks(TrackList {
        uri: "spotify:playlist:p1".into(),
        tracks,
        total: 110,
        complete: false,
    }));
    assert!(!app.loading_more);
    assert_eq!(app.tracks_cursor.selected, selected);
    assert_eq!(press(&mut app, KeyCode::PageDown), Action::None);
}

#[test]
fn a_fully_loaded_list_never_asks_for_more() {
    let mut app = in_playlist(110);
    for _ in 0..200 {
        assert_eq!(press(&mut app, KeyCode::Down), Action::None);
    }
    assert_eq!(app.tracks_cursor.selected, 109);
}

#[test]
fn a_failed_page_can_be_asked_for_again() {
    let mut app = in_playlist(50);
    app.tracks_cursor.move_by(45, 50);
    let more = send(Request::MoreTracks {
        uri: "spotify:playlist:p1".into(),
    });
    assert_eq!(press(&mut app, KeyCode::Down), more);
    app.apply(Message::Notice {
        text: "Spotify is rate limiting requests; trying again in 30s".into(),
    });
    assert!(!app.loading_more);
    assert_eq!(press(&mut app, KeyCode::Down), more);
}

#[test]
fn tracks_for_a_list_that_is_no_longer_open_are_dropped() {
    let mut app = in_playlist(50);
    press(&mut app, KeyCode::Esc);
    let late = TrackList {
        uri: "spotify:playlist:p1".into(),
        ..TrackList::default()
    };
    app.apply(Message::Tracks(late));
    assert!(app.tracks.is_none());

    let mut app = in_playlist(50);
    let other = TrackList {
        uri: "spotify:playlist:p3".into(),
        ..TrackList::default()
    };
    app.apply(Message::Tracks(other));
    assert_eq!(app.tracks.as_ref().map(|list| list.tracks.len()), Some(50));
}

#[test]
fn search_sends_one_request_on_enter_and_none_while_typing() {
    let mut app = browsing();
    assert_eq!(press(&mut app, KeyCode::Char('/')), Action::None);
    assert!(app.search.editing);
    // Letters that are shortcuts elsewhere are just letters here.
    type_text(&mut app, "q snr ");
    assert_eq!(app.search.query, "q snr ");
    press(&mut app, KeyCode::Backspace);
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        send(Request::Search {
            query: "q snr".into()
        })
    );
    assert!(app.search.waiting && !app.search.editing);
}

#[test]
fn an_empty_query_is_not_sent_and_esc_leaves_search() {
    let mut app = browsing();
    press(&mut app, KeyCode::Char('/'));
    type_text(&mut app, "  ");
    assert_eq!(press(&mut app, KeyCode::Enter), Action::None);
    assert!(app.search.editing);
    assert_eq!(press(&mut app, KeyCode::Esc), Action::None);
    assert_eq!(app.screen, Screen::Library);
}

#[test]
fn only_the_answer_to_the_current_query_is_shown() {
    let mut app = browsing();
    press(&mut app, KeyCode::Char('/'));
    type_text(&mut app, "new");
    press(&mut app, KeyCode::Enter);
    app.apply(Message::SearchResults(SearchResults {
        query: "old".into(),
        ..SearchResults::default()
    }));
    assert!(app.search.results.is_none() && app.search.waiting);
    app.apply(Message::SearchResults(SearchResults {
        query: "new".into(),
        ..SearchResults::default()
    }));
    assert!(app.search.results.is_some() && !app.search.waiting);
}

#[test]
fn a_found_song_plays_with_the_other_found_songs_and_a_found_playlist_plays_or_opens() {
    let mut app = searched();
    press(&mut app, KeyCode::Down);
    let uris = vec!["spotify:track:1".to_owned(), "spotify:track:7".to_owned()];
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        send(Request::PlayTracks { uris, index: 1 })
    );
    assert_eq!(
        press(&mut app, KeyCode::Right),
        Action::None,
        "a song cannot be opened"
    );

    press(&mut app, KeyCode::Down);
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        play_context("spotify:playlist:p9", None)
    );
    assert_eq!(
        press(&mut app, KeyCode::Right),
        send(Request::GetTracks {
            uri: "spotify:playlist:p9".into()
        })
    );

    // Back from that playlist leads to the results, then to the library.
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.screen, Screen::Search);
    assert_eq!(app.search.cursor.selected, 2);
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.screen, Screen::Library);
}

#[test]
fn transport_keys_work_on_every_screen() {
    for mut app in [browsing(), in_playlist(50), searched()] {
        assert_eq!(press(&mut app, KeyCode::Char(' ')), send(Request::Toggle));
        assert_eq!(press(&mut app, KeyCode::Char('n')), send(Request::Next));
        assert_eq!(press(&mut app, KeyCode::Char('p')), send(Request::Previous));
        assert_eq!(
            press(&mut app, KeyCode::Char('.')),
            send(Request::Seek {
                position_ms: 25_000
            })
        );
        assert_eq!(
            press(&mut app, KeyCode::Char(',')),
            send(Request::Seek { position_ms: 5_000 })
        );
        assert_eq!(
            press(&mut app, KeyCode::Char('+')),
            send(Request::Volume { percent: 55 })
        );
        assert_eq!(
            press(&mut app, KeyCode::Char('-')),
            send(Request::Volume { percent: 45 })
        );
        assert_eq!(
            press(&mut app, KeyCode::Char('s')),
            send(Request::Shuffle { on: true })
        );
        assert_eq!(
            press(&mut app, KeyCode::Char('r')),
            send(Request::Repeat {
                mode: Repeat::Context
            })
        );
        assert_eq!(press(&mut app, KeyCode::Char('q')), Action::Quit);
    }
}

#[test]
fn with_nothing_loaded_space_starts_the_selection() {
    let mut idle = ready();
    assert_eq!(
        press(&mut idle, KeyCode::Char(' ')),
        play_context("spotify:user:allar:collection", None)
    );
    assert_eq!(
        press(&mut idle, KeyCode::Char('.')),
        Action::None,
        "nothing to seek in"
    );
}

#[test]
fn seeking_and_volume_stop_at_their_limits_and_seeking_follows_playback() {
    let mut app = playing();
    app.state.position_ms = 188_000;
    assert_eq!(
        press(&mut app, KeyCode::Char('.')),
        send(Request::Seek {
            position_ms: 191_000
        })
    );
    app.state.position_ms = 3_000;
    assert_eq!(
        press(&mut app, KeyCode::Char(',')),
        send(Request::Seek { position_ms: 0 })
    );
    app.state.volume = 98;
    assert_eq!(
        press(&mut app, KeyCode::Char('=')),
        send(Request::Volume { percent: 100 })
    );
    app.state.volume = 2;
    assert_eq!(
        press(&mut app, KeyCode::Char('-')),
        send(Request::Volume { percent: 0 })
    );

    let mut app = playing();
    let forward = KeyEvent::new(KeyCode::Char('.'), KeyModifiers::NONE);
    assert_eq!(
        handle_key(&mut app, forward, NOW + 20_000),
        send(Request::Seek {
            position_ms: 45_000
        })
    );
}

#[test]
fn without_a_player_only_enter_and_close_do_anything() {
    let mut app = App {
        link: Link::Up,
        ..App::default()
    };
    app.state.status = Status::SignedOut;
    assert_eq!(press(&mut app, KeyCode::Char(' ')), Action::None);
    assert_eq!(press(&mut app, KeyCode::Char('/')), Action::None);
    assert_eq!(press(&mut app, KeyCode::Enter), send(Request::Retry));
    app.state.status = Status::Connecting;
    assert_eq!(press(&mut app, KeyCode::Enter), Action::None);

    let mut down = App {
        link: Link::Down("gone".into()),
        ..App::default()
    };
    assert_eq!(press(&mut down, KeyCode::Enter), Action::Reconnect);
    let mut starting = App::default();
    assert_eq!(press(&mut starting, KeyCode::Enter), Action::None);

    for app in [&mut app, &mut down, &mut starting] {
        assert_eq!(press(app, KeyCode::Esc), Action::Quit);
        assert_eq!(press(app, KeyCode::Char('q')), Action::Quit);
        let control_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(handle_key(app, control_c, NOW), Action::Quit);
    }
}

#[test]
fn control_c_closes_even_while_typing_a_query() {
    let mut app = browsing();
    press(&mut app, KeyCode::Char('/'));
    let control_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(handle_key(&mut app, control_c, NOW), Action::Quit);
}
