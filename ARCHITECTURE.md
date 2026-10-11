# Architecture

How the code is laid out, and where a change of each kind goes. `PRODUCT.md` says what the plugin is for and `DESIGN.md` how it looks; this file is about the code.

## Layout

```text
src/
  main.rs            run the CLI, report an error
  cli.rs             every command; the first client of the socket
  protocol.rs        the wire format: Message (daemon to client), Request, State and friends
  client.rs          connect to the daemon, start it, replace one from another build
  paths.rs           every file in the state directory
  config.rs          config.toml: the Spotify developer app
  credentials.rs     the two refresh tokens, as private files
  login.rs           the audio sign-in, then the browsing one
  daemon.rs          lock, socket, signals, the Hub that joins server, player and web
  daemon/
    server.rs        one task per client: state now, every change after, requests routed
    player.rs        the librespot session: connect, reconnect, events in, state out
    playback.rs      requests to player commands; resumes when nothing is loaded
    resume.rs        where playback was; how to load it back
    state.rs         how a player event changes State (pure)
    cover.rs         fetch and prune cover images
    web.rs           answers browsing requests from the catalog, one at a time
  web.rs             Api::get, the one gate to the Web API: token, 401, 429 cooldown
  web/
    auth.rs          PKCE sign-in and token refresh
    catalog.rs       library, track pages, search; what is cached and for how long
    cache.rs         cached answers on disk
  ui.rs              App, screens, apply a message, draw, the event loop
  ui/
    keys.rs          what each key does, per screen
    list.rs          the one list component and its cursor
    library.rs       library entries and rows
    browse.rs        track rows and search rows
    player.rs        the docked player strip
    cover.rs         cover art as half blocks
    chrome.rs, text.rs, theme.rs
    tests.rs, tests/ golden snapshots and key behaviour
tests/               through the real binary, the socket server, and the Web API gate
tests/common/        sandbox, a stand-in Spotify that counts requests
tests/snapshots/ui/  golden snapshots of every popup state
scripts/regress      the regression gate
```

## Rules that hold everywhere

- **The daemon owns the truth.** The popup and CLI draw what it pushes and send requests. They never talk to Spotify and never keep player state of their own.
- **Player state never comes from the Web API.** It comes from librespot's events, through `daemon/state.rs`.
- **One gate to the Web API.** Every request goes through `Api::get`. One at a time, no automatic retries, nothing sent during a 429 cooldown. A new kind of data states its budget in `web/catalog.rs` and gets a counting test.
- **Nothing is loaded into librespot until the user asks to play.** Starting the daemon must never take playback from another device.
- **State is pushed, not polled.** Clients get the state on connect and each change after. A change that alters nothing visible sends nothing.
- **Secrets stay in two files.** Tokens live in `credentials.rs`'s files, mode 0600. Never in logs, snapshots, the config directory or the repository. The gate greps for tokens in log calls.
- **File paths come from `paths.rs`.** Tests point `HERDR_PLUGIN_STATE_DIR` and `HERDR_PLUGIN_CONFIG_DIR` at a sandbox; nothing may reach the real directories another way.
- **Names from Spotify never become file names unchecked.** `cover.rs` and `cache.rs` accept only plain IDs.
- **Build with `--locked`.** The lockfile pins `vergen` 9.0.6 for librespot-core 0.8.0's build script. After adding a dependency, check the pin survived.
- **Rust files stay under 500 lines.** The gate enforces it; split by responsibility.
- **Appearance follows `DESIGN.md`**, which inherits the Personal Inbox's. Ratatui emits colour roles, never hex values; the cover is the one exception.

## The protocol

Newline-delimited JSON over `daemon.sock`. The daemon opens with `Hello` (protocol number, build stamp, pid) and the current `State`.

- Playback requests go to the player task; browsing requests (`get_library`, `get_tracks`, `more_tracks`, `search`) go to the web task. `server.rs` routes them.
- Browsing answers and notices are broadcast to every client. A client ignores answers it did not ask for (`ui.rs` checks the list URI and the query).
- A failed request answers with a `Notice`, never with silence.
- A client whose build stamp differs from the daemon's stops the daemon and starts its own (`client.rs`). Bump `PROTOCOL` on an incompatible change; a rebuilt binary is replaced anyway.

## Adding things

### A playback request

1. Add the variant to `Request` in `protocol.rs`.
2. Handle it in `daemon/playback.rs`: in `command` when something is loaded, and decide in `before_loading` what it means when nothing is.
3. Add a `ctl` action in `cli.rs` if it is useful from a shell, and a key in `ui/keys.rs`.

### A piece of player state

1. Add the field to `State`.
2. Add a `Change` and its arm in `daemon/state.rs`, with a test.
3. Map the librespot event in `player.rs::report`. If it should survive a restart, add it to `Resume`.

### Something fetched from the Web API

1. Add the fetch to `web/catalog.rs` with its cache rule, and state the cost in the module comment.
2. Add the request and answer to `protocol.rs`, the arm to `daemon/web.rs`, and the routing in `daemon/server.rs`.
3. Add a counting test in `tests/web_budget.rs` and, for the daemon's answer, `tests/web_daemon.rs`.
4. Update the budget table in `README.md`.

### A screen

1. Add a variant to `Screen` in `ui.rs`; the compiler lists every `match` needing an arm.
2. Build its rows as `list::Row`s and draw them with `list::draw`. Add its hints.
3. Add its keys to `ui/keys.rs`.
4. Add snapshot scenarios in `ui/tests.rs` and key tests in `ui/tests/keys.rs`.

### A key

Add it to `ui/keys.rs`: in `transport` if it means the same everywhere, otherwise in the screen's function. Add it to the hints in `ui.rs`, the table in `README.md`, and a test.

## Tests

- **Pure rules** are tested where they live: `daemon/state.rs`, `daemon/resume.rs`, `web/auth.rs`, `web/catalog.rs`.
- **Golden snapshots** (`src/ui/tests.rs`, files in `tests/snapshots/ui/`) pin how every state looks, text and styling, at three sizes. Do not assert on rendered text elsewhere.
- **Key behaviour** (`src/ui/tests/keys.rs`) pins what keys do to the app and which request they send.
- **The socket server** (`tests/server.rs`) runs against a stand-in player.
- **The Web API budget** (`tests/web_budget.rs`, `tests/web_daemon.rs`) counts requests at a stand-in Spotify.
- **The daemon as a process** (`tests/daemon_process.rs`) drives the real binary: lazy start, single instance, replacement, idle exit, signals, resume, browsing end to end.
- **Not covered:** the librespot session and audio output. Check those by hand after touching `player.rs` or `playback.rs`: play, pause, skip, stop the daemon, resume.

Test one behaviour at one layer.
