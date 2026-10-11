//! Golden snapshots of every popup state, text plus styling, at three sizes; and what keys do.
//!
//! Snapshots live in `tests/snapshots/ui/`. Record intended changes with
//! `scripts/regress --update-snapshots` and review them through `git diff`.

use super::*;
use crate::protocol::Track;
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

#[test]
fn snapshot_playing_with_cover() {
    check("playing", &mut playing());
}

#[test]
fn snapshot_paused_before_the_cover_arrives() {
    let mut app = playing();
    app.state.playing = false;
    app.state.shuffle = true;
    app.state.repeat = Repeat::Track;
    let track = app.state.track.as_mut().expect("track");
    track.cover_path = None;
    track.name =
        "A Remarkably Long Track Title That Cannot Possibly Fit On One Line (Live, Remastered)"
            .into();
    check("paused", &mut app);
}

#[test]
fn snapshot_nothing_playing() {
    check("nothing-playing", &mut ready());
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
    let mut app = playing();
    app.notify("Track unavailable: spotify:track:2".into());
    check("notice", &mut app);
}

#[test]
fn every_state_draws_at_any_size_without_panicking() {
    let mut apps = vec![playing(), ready(), App::default()];
    apps.push(App {
        link: Link::Down("gone".into()),
        ..App::default()
    });
    for app in &mut apps {
        for width in [0, 1, 4, 11, 20, 56, 57, 200] {
            for height in [0, 1, 3, 5, 9, 14, 60] {
                render(app, (width, height));
            }
        }
    }
}

fn press(app: &mut App, code: KeyCode) -> Action {
    handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), NOW)
}

fn send(request: Request) -> Action {
    Action::Send(request)
}

#[test]
fn enter_plays_the_selected_row_and_space_toggles() {
    let mut app = playing();
    let liked = Request::PlayContext {
        uri: "spotify:user:allar:collection".into(),
        index: None,
    };
    assert_eq!(press(&mut app, KeyCode::Enter), send(liked.clone()));
    assert_eq!(press(&mut app, KeyCode::Char(' ')), send(Request::Toggle));

    // With nothing loaded, Space has nothing to resume and starts the selection instead.
    let mut idle = ready();
    assert_eq!(press(&mut idle, KeyCode::Char(' ')), send(liked));
}

#[test]
fn transport_keys_send_one_request_each() {
    let mut app = playing();
    assert_eq!(press(&mut app, KeyCode::Char('n')), send(Request::Next));
    assert_eq!(press(&mut app, KeyCode::Char('p')), send(Request::Previous));
    assert_eq!(
        press(&mut app, KeyCode::Right),
        send(Request::Seek {
            position_ms: 25_000
        })
    );
    assert_eq!(
        press(&mut app, KeyCode::Left),
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
}

#[test]
fn seeking_and_volume_stop_at_their_limits() {
    let mut app = playing();
    app.state.position_ms = 188_000;
    assert_eq!(
        press(&mut app, KeyCode::Right),
        send(Request::Seek {
            position_ms: 191_000
        })
    );
    app.state.position_ms = 3_000;
    assert_eq!(
        press(&mut app, KeyCode::Left),
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
}

#[test]
fn seeking_follows_the_position_as_it_plays() {
    let mut app = playing();
    let right = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
    let action = handle_key(&mut app, right, NOW + 20_000);
    assert_eq!(
        action,
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
    assert_eq!(press(&mut app, KeyCode::Char('n')), Action::None);
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
fn a_pushed_state_replaces_what_is_shown_and_a_notice_is_kept() {
    let mut app = ready();
    let mut next = playing().state;
    next.volume = 80;
    app.apply(Message::State(next.clone()));
    assert_eq!(app.state, next);
    app.apply(Message::Notice {
        text: "Track unavailable".into(),
    });
    assert_eq!(
        app.notice.as_ref().map(|(text, _)| text.as_str()),
        Some("Track unavailable")
    );
}
