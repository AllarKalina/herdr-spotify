# Herdr Spotify

<!-- impeccable:product-schema 1 -->

## Users

Allar, at one of two Macs, working in Herdr with agents running in other panes. Music is background to the work. He listens on the computer he is sitting at and never steers playback from another device.

## Product Purpose

Put music on without leaving the terminal. One key opens a popup; he picks a playlist or searches, presses play, and closes it. The music keeps playing after the popup is gone. Success is choosing what to hear in a few keystrokes and getting back to work.

## Positioning

A local Spotify player inside Herdr. This computer plays the audio itself through a small background daemon, so the popup shows the true player state instantly and makes no Spotify API calls to do it. It is not a remote control for other devices.

## Operating Context

- Runs as a macOS terminal popup inside Herdr, opened with `cmd+m`, beside the Personal Inbox plugin (`cmd+i`). The two popups cannot be open at once.
- The popup process exists only while it is open. A daemon started on first use plays the audio and holds all state; it exits after 15 minutes with nothing playing and no popup open.
- Keyboard first. Mouse support follows the Inbox where it is cheap.
- A CLI (`herdr-spotify status`, `ctl`, `login`, `daemon stop`) drives the same daemon from a shell.
- Each Mac signs in separately with `herdr-spotify login`. One Spotify Premium account, one stream at a time.

## Capabilities and Constraints

- Built: the daemon, both sign-ins, transport (play, pause, next, previous, seek, volume, shuffle, repeat), the library of playlists and Liked Songs, a track view that loads 50 tracks at a time as you scroll, search for songs and playlists, cover art, and resuming where playback stopped after the daemon restarts.
- Not built yet: mouse support, albums and artists in search.
- A new daemon shows the last track paused at its position. The first press of play, or of next or previous, loads it back with the same playlist, volume, shuffle and repeat; until then this computer does not take playback away from another device.
- Spotify Web API calls happen only when needed, under a tested call budget; player state never comes from the Web API.
- The audio sign-in is a refresh token in a file only the user can read, in the plugin's state directory. No hosted backend, no sync between the two Macs.
- Playback uses librespot, an unofficial Spotify client library. Spotify can break it; the popup must say plainly when playback is unavailable.
- Terminology: "Liked Songs", "playlist", "this computer". Not "device", "transfer" or "Connect".
- Cover art is drawn with coloured half-block characters; it is coarse by nature.

## Brand Commitments

Part of the same family as the Personal Inbox: same header position, footer shortcut line, insets and teal accent, on the terminal's own translucent background. Quiet and quick; it is a tool glanced at between other things.

## Evidence on Hand

- The daemon, protocol, CLI and tests are in this repository. The design spec is local and not committed.
- The Inbox's `DESIGN.md` (`https://github.com/AllarKalina/herdr-inbox/blob/main/DESIGN.md`) is the visual authority this plugin inherits.
- `Clipify` is the earlier Spotify terminal app whose screens (library sidebar, track list, bottom player, search) informed this one. No code is shared.

## Product Principles

- Choosing music comes first; the player is a strip, not the page.
- Show what is true now. Never show a stale or guessed player state.
- Every state says what to do next: sign in, retry, or pick something.
- Stay out of the way: few keys, no confirmations for reversible actions, no settings until one is needed.
- Words beside every icon; colour is a supporting cue.
