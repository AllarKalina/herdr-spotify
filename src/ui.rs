//! The popup: lists of things to play with the player docked beneath them.
//!
//! The popup holds no truth of its own. It draws what the daemon pushes and turns keys into
//! requests; it never talks to Spotify.

mod browse;
mod chrome;
pub mod cover;
mod keys;
pub mod library;
pub mod list;
mod player;
mod text;
mod theme;

#[cfg(test)]
mod tests;

pub use keys::handle_key;

use crate::Result;
use crate::client::{Connection, Sender};
use crate::paths::Paths;
use crate::protocol::{Library, Message, Request, SearchResults, State, Status, TrackList, now_ms};
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use list::Cursor;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use std::io::stdout;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

const NOTICE_TIME: Duration = Duration::from_secs(6);
/// How often the loop looks for daemon messages while no key arrives.
const TICK: Duration = Duration::from_millis(100);

const HINTS_LIBRARY: [&str; 4] = [
    "Enter play · → open · / search · Space pause · n/p skip · ,/. seek · -/+ volume · Esc close",
    "Enter play · → open · / search · Space pause · n/p skip · Esc close",
    "↵ play · → open · / search · Esc close",
    "↵ play · → open · / search",
];
const HINTS_TRACKS: [&str; 4] = [
    "Enter play · ← back · / search · Space pause · n/p skip · ,/. seek · -/+ volume · Esc back",
    "Enter play · ← back · / search · Space pause · n/p skip · Esc back",
    "↵ play · ← back · / search · Esc back",
    "↵ play · ← back · / search",
];
const HINTS_RESULTS: [&str; 3] = [
    "Enter play · → open playlist · / new search · Space pause · n/p skip · Esc back",
    "↵ play · → open · / new search · Esc back",
    "↵ play · → open · Esc back",
];
const HINTS_QUERY: [&str; 1] = ["Enter search · Esc back"];
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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Screen {
    #[default]
    Library,
    Tracks(Open),
    Search,
}

/// The list whose tracks are on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Open {
    pub title: String,
    /// The Spotify context that plays it.
    pub play_uri: String,
    /// What the daemon is asked for to list it.
    pub tracks_uri: String,
    /// Where Esc leads: back to search results rather than the library.
    pub from_search: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Search {
    pub query: String,
    /// True while the query is being typed; then every key types.
    pub editing: bool,
    pub waiting: bool,
    pub results: Option<SearchResults>,
    pub cursor: Cursor,
}

pub struct App {
    pub state: State,
    pub link: Link,
    pub screen: Screen,
    pub library: Option<Library>,
    pub library_cursor: Cursor,
    /// The tracks of the open list, once they have arrived.
    pub tracks: Option<TrackList>,
    pub tracks_cursor: Cursor,
    /// True from asking for the next page until it, or a failure, arrives.
    pub loading_more: bool,
    pub search: Search,
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
            screen: Screen::Library,
            library: None,
            library_cursor: Cursor::default(),
            tracks: None,
            tracks_cursor: Cursor::default(),
            loading_more: false,
            search: Search::default(),
            notice: None,
            cover: cover::Cache::default(),
        }
    }
}

impl App {
    pub fn notify(&mut self, text: String) {
        self.notice = Some((text, Instant::now()));
    }

    pub fn apply(&mut self, message: Message) {
        match message {
            Message::State(state) => self.state = state,
            Message::Library(library) => self.library = Some(library),
            Message::Tracks(list) => {
                // An answer for a list that is no longer open is dropped.
                if matches!(&self.screen, Screen::Tracks(open) if open.tracks_uri == list.uri) {
                    self.tracks_cursor.clamp(list.tracks.len());
                    self.tracks = Some(list);
                    self.loading_more = false;
                }
            }
            Message::SearchResults(results) => {
                if results.query == self.search.query.trim() {
                    self.search.results = Some(results);
                    self.search.waiting = false;
                    self.search.cursor = Cursor::default();
                }
            }
            Message::Notice { text } => {
                // A notice is how a failed request answers; stop waiting for it.
                self.loading_more = false;
                self.search.waiting = false;
                self.notify(text);
            }
            Message::Hello { .. } => {}
        }
        let entries = library::entries(&self.state, self.library.as_ref()).len();
        self.library_cursor.clamp(entries);
    }

    /// True when the lists and the player can be used.
    fn ready(&self) -> bool {
        self.link == Link::Up && self.state.status == Status::Ready
    }

    /// Opens a list's tracks and returns the request that fetches them.
    fn open(&mut self, entry: &library::Entry, from_search: bool) -> Action {
        self.screen = Screen::Tracks(Open {
            title: entry.name.trim_start_matches("♥ ").to_owned(),
            play_uri: entry.play_uri.clone(),
            tracks_uri: entry.tracks_uri.clone(),
            from_search,
        });
        self.tracks = None;
        self.tracks_cursor = Cursor::default();
        self.loading_more = false;
        Action::Send(Request::GetTracks {
            uri: entry.tracks_uri.clone(),
        })
    }
}

pub fn draw(frame: &mut Frame, app: &mut App, now_ms: u64) {
    let crumb = match &app.screen {
        Screen::Library => None,
        Screen::Tracks(open) => Some(open.title.clone()),
        Screen::Search => Some("Search".to_owned()),
    };
    chrome::draw_header(frame, crumb.as_deref().filter(|_| app.ready()));
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
        let mut area = Rect::new(
            body.x,
            body.y,
            body.width,
            body.height.saturating_sub(strip_height + 1),
        );
        let playing = app.state.playing;
        let hints: &[&str] = match &app.screen {
            Screen::Library => {
                let rows = library::rows(&app.state, app.library.as_ref());
                list::draw(frame, area, &rows, &mut app.library_cursor, playing);
                &HINTS_LIBRARY
            }
            Screen::Tracks(open) => {
                let rows =
                    browse::track_rows(open, app.tracks.as_ref(), app.loading_more, &app.state);
                list::draw(frame, area, &rows, &mut app.tracks_cursor, playing);
                &HINTS_TRACKS
            }
            Screen::Search => {
                let query = text::fit(&browse::query_line(&app.search), usize::from(area.width));
                frame.render_widget(
                    Paragraph::new(query),
                    Rect::new(area.x, area.y, area.width, 1.min(area.height)),
                );
                area = Rect::new(
                    area.x,
                    area.y + 2.min(area.height),
                    area.width,
                    area.height.saturating_sub(2),
                );
                let rows = browse::search_rows(&app.search, &app.state);
                list::draw(frame, area, &rows, &mut app.search.cursor, playing);
                if app.search.editing {
                    &HINTS_QUERY
                } else {
                    &HINTS_RESULTS
                }
            }
        };
        let cover_path = app
            .state
            .track
            .as_ref()
            .and_then(|track| track.cover_path.as_deref());
        let art = app.cover.get(cover_path).cloned();
        player::draw(frame, strip, &app.state, art.as_ref(), now_ms);
        chrome::draw_footer(frame, hints);
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
                Incoming::Connected(mut connected) => {
                    // The daemon answers from its cache unless that is stale.
                    let _ = connected.send(&Request::GetLibrary { refresh: false });
                    if let Screen::Tracks(open) = &app.screen {
                        let _ = connected.send(&Request::GetTracks {
                            uri: open.tracks_uri.clone(),
                        });
                    }
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
                    app.loading_more = false;
                    app.search.waiting = false;
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
