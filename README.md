# Personal Spotify

A Spotify player in a Herdr popup. One key opens it; you pick a playlist or search, press play, and close it. The music keeps playing: this computer plays the audio itself through a small background daemon.

It is a local player, not a remote control. It does not show or steer playback on your phone or another computer.

## Requirements

- macOS, Herdr **0.9.1 or newer**, Rust **1.89 or newer** with `cargo` on `PATH`, and Xcode Command Line Tools.
- Spotify **Premium**.
- For playlists and search: a Spotify developer app (free, from the Spotify developer dashboard). Without one, Liked Songs still plays.

## Install on a Mac

```sh
herdr plugin install AllarKalina/herdr-spotify --ref main
herdr plugin list --plugin personal.spotify
```

Herdr clones the repository and runs the manifest's locked release build for that Mac's architecture. It does not install Rust or other prerequisites. The first build takes about a minute.

To work on the plugin, link a local clone instead:

```sh
git clone https://github.com/AllarKalina/herdr-spotify.git
cd herdr-spotify
cargo build --release --locked
herdr plugin link "$PWD"
```

A linked copy must be unlinked before that Mac switches to a managed install.

Always build with `--locked`, as the manifest does. The lockfile pins a build dependency (`vergen` 9.0.6) that librespot 0.8.0 needs; a fresh resolve does not compile.

Each Mac installs and signs in separately. Do not copy Herdr's plugin registry, the compiled binary, or the state directory between computers.

### Key binding

Add to Herdr's `config.toml`, then run `herdr server reload-config`:

```toml
[[keys.command]]
key = "cmd+shift+m"
type = "plugin_action"
command = "personal.spotify.open"
description = "Open Spotify player"
```

`cmd+m` cannot be used: it is macOS's Minimize command and Ghostty has no binding for it to release.

### Sign in

```sh
~/.config/herdr/plugins/github/personal.spotify-*/target/release/herdr-spotify login
```

That is the managed install's binary. With a local clone it is `target/release/herdr-spotify login`, run from the clone.

The browser opens twice, once for each sign-in:

1. **Audio.** Through librespot's own Spotify client. Nothing to configure.
2. **Playlists and search.** Through your developer app, read-only. This step is skipped until the app is configured.

To configure the developer app, put its client ID and one of its redirect URIs in `config.toml` in the plugin's config directory (`herdr plugin config-dir personal.spotify`):

```toml
client_id = "your app's client ID"
redirect_uri = "http://127.0.0.1:3000/callback"
```

- The redirect URI must be registered on the app in the Spotify dashboard, character for character, and must be `http://127.0.0.1:<port>/<path>`. Spotify does not accept `localhost`.
- Nothing else may be listening on that port while you sign in.
- The client ID is not a secret. There is no client secret; do not add one.

Run `login` again after changing the config. Signing in again is also the fix when the popup says a sign-in is missing or expired.

## Using the popup

| Key | Does |
|---|---|
| `j` `k` or arrows, PageUp, PageDown | Move the selection |
| Enter | Play the selected playlist, or the selected song inside its playlist |
| Right or `l` | Open a playlist's songs |
| Left, `h` or Esc | Back one level; Esc closes the popup from the library |
| `/` | Search. Type, then Enter. Nothing is sent while you type |
| Space | Pause or resume |
| `n` `p` | Next, previous |
| `,` `.` | Seek back or forward ten seconds |
| `-` `+` | Volume |
| `s` `r` | Shuffle; repeat off, all, one |
| `R` | Fetch the library from Spotify again |
| `q` | Close |

Long lists load 50 songs at a time as you scroll. A `▶` marks what is loaded in the player.

A new daemon shows the last track paused where it stopped. Space resumes it with the same playlist, volume, shuffle and repeat. Opening the popup never starts playback or takes it from another device.

## Command line

The same daemon can be driven from a shell:

```sh
herdr-spotify status [--json]         # what is playing
herdr-spotify ctl toggle              # also: play, pause, next, previous
herdr-spotify ctl seek 90             # seconds
herdr-spotify ctl volume 40
herdr-spotify ctl play-context spotify:playlist:<id> [index | track-uri]
herdr-spotify library [--refresh]     # playlists with their URIs
herdr-spotify tracks <playlist-uri|liked> [--more]
herdr-spotify search <words>
herdr-spotify daemon stop
```

`herdr-spotify tui` runs the interface in any terminal, outside the popup.

## How it works

One binary, two processes:

- **The daemon** plays audio through [librespot](https://github.com/librespot-org/librespot) and owns every conversation with Spotify. The first command or popup starts it; it exits after 15 minutes with nothing playing and nothing connected.
- **The popup** and the CLI talk to the daemon over a Unix socket. The daemon pushes the player state when it changes; nothing polls.

While the daemon runs, this computer appears in other Spotify apps as `Herdr · <computer name>`.

### Spotify Web API use

Player state never comes from the Web API. It is used only for browsing, under a fixed budget:

| Data | Requests | When |
|---|---|---|
| Playlists and the Liked Songs count | 2, plus 1 per 50 playlists | Cache older than 30 minutes, or `R` |
| Opening a list | 1 | First time, or after the playlist changed |
| Each further 50 songs | 1 | When you scroll near the end of what is loaded |
| Liked Songs after 30 minutes | 1 | Confirms nothing changed; loaded pages are kept |
| A search | 1 | Per query, reused for 5 minutes |

Requests go out one at a time and are never retried automatically. After a rate-limit answer nothing is sent until the wait Spotify asked for has passed, even if the daemon restarts; the popup keeps showing cached data.

### Files

Everything is in Herdr's state directory for the plugin, `~/.local/state/herdr/plugins/personal.spotify/`, readable by you only:

| File | Holds |
|---|---|
| `audio-credentials`, `web-credentials` | The two refresh tokens |
| `daemon.log` | The daemon's log; start here when something is wrong |
| `resume.json` | Where playback stopped |
| `cache/`, `covers/` | Playlists and track lists as last fetched; recent cover images |
| `daemon.sock`, `daemon.lock`, `device-id`, `web-cooldown` | Runtime state |

Deleting the directory signs this computer out and clears the caches. The tokens are plain files, not Keychain items: the binary is unsigned, and macOS would otherwise ask for your login password after every rebuild.

## Troubleshooting

- **"Not signed in" or "Playback is unavailable".** Run `login` again, then press Enter in the popup. `daemon.log` has the reason.
- **The browsing sign-in fails to start.** Something else is using the port in `redirect_uri`, or the URI is not registered on the developer app.
- **"Spotify is rate limiting requests".** Wait the stated time. Cached lists still work.
- **The popup shows stale state after a rebuild.** It should not: a rebuilt binary replaces the running daemon on the next command. `herdr-spotify daemon stop` forces it.
- **The manifest changed** in a linked clone. Run `herdr plugin link` on the clone again and reopen the popup.

## Limits

- librespot is an unofficial client. If Spotify changes its protocol, playback can stop working until librespot is updated. Browsing does not depend on it.
- One stream per account: starting playback here stops it elsewhere, and the other way round.
- Search returns songs and playlists, not albums or artists.
- No mouse support. No playlist editing, podcasts or lyrics.

## Development

```sh
scripts/regress                     # every check; done means PASS
scripts/regress --quick             # skip the release build and smoke run
scripts/regress --update-snapshots  # re-record UI snapshots after an intended UI change
```

The gate runs formatting, lints, the tests, UI snapshot comparison, a 500-line limit per Rust file, a check that no token reaches a log, the release build and a smoke run. It needs no Spotify account: the Web API is replaced by a stand-in that counts requests, which is how the budget above is tested. Audio output is checked by hand.

`tests/snapshots/ui/` holds a snapshot of every popup state at 100×35, 60×24 and 40×18. `PRODUCT.md` says what the plugin is for, `DESIGN.md` how it looks, and [ARCHITECTURE.md](ARCHITECTURE.md) how the code is laid out and where each kind of change goes.
