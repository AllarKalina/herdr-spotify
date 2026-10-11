//! The popup: a list of things to play with the player docked beneath it.
//!
//! The popup holds no truth of its own. It draws the state the daemon pushes and turns keys
//! into requests; it never talks to Spotify.

mod chrome;
pub mod cover;
mod library;
mod player;
mod text;
mod theme;

#[cfg(test)]
mod tests;

use crate::Result;
use crate::client::{Connection, Sender};
use crate::paths::Paths;
use crate::protocol::{Message, Repeat, Request, State, Status, now_ms};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use std::io::stdout;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

const SEEK_STEP_MS: u32 = 10_000;
const VOLUME_STEP: u8 = 5;
const NOTICE_TIME: Duration = Duration::from_secs(6);
/// How often the loop looks for daemon messages while no key arrives.
const TICK: Duration = Duration::from_millis(100);

const HINTS_PLAYER: [&str; 4] = [
    "Enter play · Space pause · n/p skip · ←/→ seek · -/+ volume · s shuffle · r repeat · Esc close",
    "Enter play · Space pause · n/p skip · ←/→ seek · -/+ volume · Esc close",
    "↵ play · Space pause · n/p skip · Esc close",
    "↵ play · Space pause · Esc",
];
const HINTS_RETRY: [&str; 1] = ["Enter retry · Esc close"];
const HINTS_RESTART: [&str; 1] = ["Enter restart · Esc close"];
const HINTS_WAIT: [&str; 1] = ["Esc close"];

/// Whether the popup can reach the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    /// The daemon is being started or connected to.
    Starting,
    Up,
    /// The daemon went away, or could not be started.
    Down(String),
}

pub struct App {
    pub state: State,
    pub link: Link,
    pub selected: usize,
    notice: Option<(String, Instant)>,
    pub cover: cover::Cache,
}

/// What a key asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    Quit,
    Send(Request),
    /// Start or reach the daemon again.
    Reconnect,
}

impl Default for App {
    fn default() -> Self {
        Self {
            state: State::default(),
            link: Link::Starting,
            selected: 0,
            notice: None,
            cover: cover::Cache::default(),
        }
    }
}

impl App {
    pub fn notify(&mut self, text: String) {
        self.notice = Some((text, Instant::now()));
    }

    fn apply(&mut self, message: Message) {
        match message {
            Message::State(state) => {
                self.state = state;
                self.selected = self
                    .selected
                    .min(library::rows(&self.state).len().saturating_sub(1));
            }
            Message::Notice { text } => self.notify(text),
            Message::Hello { .. } => {}
        }
    }

    /// True when the list and the player can be used.
    fn ready(&self) -> bool {
        self.link == Link::Up && self.state.status == Status::Ready
    }
}

pub fn handle_key(app: &mut App, key: KeyEvent, now_ms: u64) -> Action {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => return Action::Quit,
        KeyCode::Char('c') if control => return Action::Quit,
        _ => {}
    }
    if app.link != Link::Up {
        let retry = key.code == KeyCode::Enter && app.link != Link::Starting;
        return if retry {
            Action::Reconnect
        } else {
            Action::None
        };
    }
    if app.state.status != Status::Ready {
        let waiting = app.state.status == Status::Connecting;
        let retry = key.code == KeyCode::Enter && !waiting;
        return if retry {
            Action::Send(Request::Retry)
        } else {
            Action::None
        };
    }

    let rows = library::rows(&app.state);
    let play_selected = |app: &App| match rows.get(app.selected) {
        Some(row) => Action::Send(Request::PlayContext {
            uri: row.uri.clone(),
            index: None,
        }),
        None => Action::None,
    };
    let state = &app.state;
    match key.code {
        KeyCode::Down | KeyCode::Char('j') => {
            app.selected = (app.selected + 1).min(rows.len().saturating_sub(1));
            Action::None
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.selected = app.selected.saturating_sub(1);
            Action::None
        }
        KeyCode::Enter => play_selected(app),
        // With nothing loaded there is nothing to resume, so Space starts the selection.
        KeyCode::Char(' ') if state.track.is_none() => play_selected(app),
        KeyCode::Char(' ') => Action::Send(Request::Toggle),
        KeyCode::Char('n') => Action::Send(Request::Next),
        KeyCode::Char('p') => Action::Send(Request::Previous),
        KeyCode::Right | KeyCode::Char('l') => seek(state, now_ms, true),
        KeyCode::Left | KeyCode::Char('h') => seek(state, now_ms, false),
        KeyCode::Char('+' | '=') => Action::Send(Request::Volume {
            percent: state.volume.saturating_add(VOLUME_STEP).min(100),
        }),
        KeyCode::Char('-') => Action::Send(Request::Volume {
            percent: state.volume.saturating_sub(VOLUME_STEP),
        }),
        KeyCode::Char('s') => Action::Send(Request::Shuffle { on: !state.shuffle }),
        KeyCode::Char('r') => Action::Send(Request::Repeat {
            mode: match state.repeat {
                Repeat::Off => Repeat::Context,
                Repeat::Context => Repeat::Track,
                Repeat::Track => Repeat::Off,
            },
        }),
        _ => Action::None,
    }
}

fn seek(state: &State, now_ms: u64, forward: bool) -> Action {
    let Some(track) = &state.track else {
        return Action::None;
    };
    let position = state.position_at(now_ms);
    let target = if forward {
        position.saturating_add(SEEK_STEP_MS).min(track.duration_ms)
    } else {
        position.saturating_sub(SEEK_STEP_MS)
    };
    Action::Send(Request::Seek {
        position_ms: target,
    })
}

pub fn draw(frame: &mut Frame, app: &mut App, now_ms: u64) {
    chrome::draw_header(frame, None);
    let content = chrome::content(frame.area());
    // The last row is the shortcut line; the one above it is kept for notices.
    let body = Rect::new(
        content.x,
        content.y,
        content.width,
        content.height.saturating_sub(2),
    );

    if app.ready() {
        let strip_height = if body.height >= player::FULL + 6 {
            player::FULL
        } else {
            player::COMPACT
        };
        let strip_height = strip_height.min(body.height);
        let strip = Rect::new(
            body.x,
            body.bottom() - strip_height,
            body.width,
            strip_height,
        );
        let list = Rect::new(
            body.x,
            body.y,
            body.width,
            body.height.saturating_sub(strip_height + 1),
        );
        library::draw(frame, list, &app.state, app.selected);
        let cover_path = app
            .state
            .track
            .as_ref()
            .and_then(|track| track.cover_path.as_deref());
        let art = app.cover.get(cover_path).cloned();
        player::draw(frame, strip, &app.state, art.as_ref(), now_ms);
        chrome::draw_footer(frame, &HINTS_PLAYER);
    } else {
        let (lines, hints): (Vec<Line>, &[&str]) = match (&app.link, app.state.status) {
            (Link::Starting, _) => (
                vec![Line::styled("Starting the player…", theme::muted())],
                &HINTS_WAIT,
            ),
            (Link::Down(problem), _) => (
                vec![
                    Line::styled("The player is not running.", theme::error()),
                    Line::styled(problem.clone(), theme::muted()),
                ],
                &HINTS_RESTART,
            ),
            (Link::Up, Status::SignedOut) => (
                vec![
                    Line::raw("Not signed in on this computer."),
                    Line::styled("In a terminal: herdr-spotify login", theme::muted()),
                    Line::styled("Then press Enter here.", theme::muted()),
                ],
                &HINTS_RETRY,
            ),
            (Link::Up, Status::Unavailable) => (
                vec![
                    Line::styled("Playback is unavailable.", theme::error()),
                    Line::styled(
                        app.state.problem.clone().unwrap_or_default(),
                        theme::muted(),
                    ),
                ],
                &HINTS_RETRY,
            ),
            (Link::Up, _) => (
                vec![Line::styled("Connecting to Spotify…", theme::muted())],
                &HINTS_WAIT,
            ),
        };
        // A problem reported by Spotify can be long; it wraps instead of being cut.
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), body);
        chrome::draw_footer(frame, hints);
    }

    if let Some((notice, _)) = &app.notice
        && content.height >= 2
    {
        let row = Rect::new(content.x, content.bottom() - 2, content.width, 1);
        let fitted = text::fit(notice, usize::from(row.width));
        frame.render_widget(Paragraph::new(fitted).style(theme::error()), row);
    }
}

/// What the connection thread tells the event loop.
enum Incoming {
    Connected(Sender),
    Message(Message),
    Lost(String),
}

/// Connects in the background, starting the daemon if needed, then forwards its messages.
fn connect(paths: &Paths) -> Receiver<Incoming> {
    let (events, incoming) = mpsc::channel();
    let paths = paths.clone();
    std::thread::spawn(move || {
        let outcome = (|| -> Result<()> {
            let mut connection = Connection::open(&paths)?;
            let _ = events.send(Incoming::Connected(connection.sender()?));
            while let Some(message) = connection.read()? {
                if events.send(Incoming::Message(message)).is_err() {
                    break;
                }
            }
            Ok(())
        })();
        let problem = outcome
            .err()
            .map_or_else(|| "It stopped.".to_owned(), |error| error.to_string());
        let _ = events.send(Incoming::Lost(problem));
    });
    incoming
}

pub fn run(paths: &Paths) -> Result<()> {
    // A panic must not strand the popup in raw mode on the alternate screen.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        default_hook(info);
    }));
    enable_raw_mode()?;
    crossterm::execute!(stdout(), EnterAlternateScreen)?;
    let result = Terminal::new(CrosstermBackend::new(stdout()))
        .map_err(Into::into)
        .and_then(|mut terminal| run_loop(&mut terminal, paths));
    restore_terminal()?;
    result
}

fn restore_terminal() -> Result<()> {
    disable_raw_mode()?;
    crossterm::execute!(stdout(), LeaveAlternateScreen)?;
    Ok(())
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    paths: &Paths,
) -> Result<()> {
    let mut app = App::default();
    let mut incoming = connect(paths);
    let mut sender: Option<Sender> = None;
    loop {
        while let Ok(event) = incoming.try_recv() {
            match event {
                Incoming::Connected(connected) => {
                    sender = Some(connected);
                    app.link = Link::Up;
                }
                Incoming::Message(message) => app.apply(message),
                Incoming::Lost(problem) => {
                    sender = None;
                    app.link = Link::Down(problem);
                }
            }
        }
        if app
            .notice
            .as_ref()
            .is_some_and(|(_, since)| since.elapsed() >= NOTICE_TIME)
        {
            app.notice = None;
        }
        terminal.draw(|frame| draw(frame, &mut app, now_ms()))?;

        if !event::poll(TICK)? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match handle_key(&mut app, key, now_ms()) {
            Action::None => {}
            Action::Quit => return Ok(()),
            Action::Send(request) => {
                let sent = sender
                    .as_mut()
                    .is_some_and(|sender| sender.send(&request).is_ok());
                if !sent {
                    app.notify("The player did not take that. Try again.".into());
                }
            }
            Action::Reconnect => {
                app.link = Link::Starting;
                incoming = connect(paths);
            }
        }
    }
}
