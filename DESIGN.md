---
name: Personal Spotify
description: A quiet list of things to play with the player docked beneath it, in the Personal Inbox's terminal world.
colors:
  teal: "#94e2d5"
  subtext-zero: "#a6adc8"
  surface-one: "#45475a"
  surface-two: "#585b70"
  red: "#f38ba8"
typography:
  terminal:
    fontFamily: "JetBrainsMono Nerd Font Mono"
  terminal-emphasis:
    fontFamily: "JetBrainsMono Nerd Font Mono"
    fontWeight: 700
spacing:
  inset-columns: 2
  header-row: 1
  cover-columns: 10
  cover-rows: 5
components:
  row-selected:
    backgroundColor: "{colors.teal}"
    textColor: "{colors.surface-one}"
    typography: "{typography.terminal-emphasis}"
  state-playing:
    textColor: "{colors.teal}"
    typography: "{typography.terminal-emphasis}"
  progress-played:
    textColor: "{colors.teal}"
  progress-remaining:
    textColor: "{colors.surface-two}"
---

# Design System: Personal Spotify

## Overview

**The list leads; the player is five rows.** The popup is opened to choose music and closed again, so the things to play take the canvas and the player sits docked above the shortcut line. It refuses the music-app habit of a large now-playing card.

This plugin inherits the Personal Inbox's world (`https://github.com/AllarKalina/herdr-inbox/blob/main/DESIGN.md`) and does not restate it: the host terminal owns the translucent background, the palette and the font; Ratatui emits colour roles, never hex values; nothing is bordered in the normal flow. The hex values above record how those roles resolve in the current Ghostty profile. Where this file is silent, the Inbox's rules apply.

## Layout

- Header at column two, row one: `Spotify` in bold teal. Inside a list or search a second segment names it, `Spotify / Focus` or `Spotify / Search`; then only the last segment is teal.
- One blank row, then the screen's list from the content inset.
- The player strip is docked at the bottom of the content area. One row below it is kept for notices, and the last row is the shortcut line.
- The strip is five rows when the popup is at least 15 content rows tall, three otherwise. The cover appears beside it from 56 content columns.

## Screens

Three screens share one list component and one set of transport keys.

- **Library.** `LIBRARY`, then Liked Songs and the playlists. Enter plays a row; Right opens its tracks.
- **Tracks.** A count line, `110 songs` or `110 songs · 50 loaded`, then the tracks. Enter plays that track inside its playlist. Left or Esc goes back. The next 50 are asked for when the selection comes within ten rows of the end; `Loading more…` then shows under the last row.
- **Search.** The query on the first row as `/ words`, with a block cursor while typing. Nothing is sent until Enter. Results are `SONGS` then `PLAYLISTS`. Esc goes back to the library.

Esc always goes back one level and closes the popup only from the library. `q` closes from anywhere except while typing a query.

## Components

### List

One component draws every list.

- A row is a name, an optional second column (artists, or a playlist's owner) from 64 columns, and a right-aligned detail (duration, or number of songs) in Subtext Zero.
- Every row reserves the same right-hand width, so columns line up whether or not a row carries the mark.
- `▶` before the detail marks what is loaded in the player: teal while playing, Subtext Zero while paused.
- Selection is the Inbox's: teal fill, Surface One text, bold, across the content width.
- Labels (`LIBRARY`, `SONGS`, a count, a remark) are Subtext Zero and never selected. A label directly above or below the selected row scrolls into view with it.
- Long names end in an ellipsis; rows never wrap.

### Player strip

- Row one: track name in bold, cut with an ellipsis; play state at the right edge, `▶ playing` in teal or `paused` in Subtext Zero. Word and icon together, as the Inbox pairs every status.
- Row two: artists and album in Subtext Zero.
- Row four: the progress line across the strip, `━` in teal for what has played and `─` in Surface Two for the rest.
- Row five: elapsed and total time at the left; volume, shuffle and repeat at the right. A setting that is on shows its value in teal. Settings drop from the right as the popup narrows: repeat first, then shuffle.
- With nothing loaded the strip is two muted lines saying so and what Enter does.

### Cover

Ten columns by five rows of `▀`, each cell carrying two pixels as foreground and background. It is the one place true colour is emitted: the cover is content, not chrome. Before the image arrives the same cells hold `░` in Surface Two, so the text never shifts.

### States without a player

Starting, connecting, signed out, unavailable and stopped replace the list and strip with two or three lines at the content inset: what is wrong in neutral or Red text, then what to do in Subtext Zero. Long problems wrap. The shortcut line names the one key that helps: `Enter retry` or `Enter restart`.

### Notices

One row above the shortcut line, in Red, for six seconds. Nothing moves to make room.

### Shortcut line

Unstyled, one line, the longest version that fits. It names what Enter and the arrows do on the current screen first, then search, then transport: `Enter play · → open · / search · Space pause · n/p skip · ,/. seek · -/+ volume · Esc close` down to `↵ play · → open · / search`. `s` shuffle, `r` repeat and `R` refresh work everywhere but are not listed.

## Do's and Don'ts

- **Do** keep every position fixed while state changes: reserve the cover, the notice row and the state word.
- **Do** say `this computer`, `Liked Songs`, `playlist`. **Don't** say device, transfer or Connect.
- **Don't** add a border, a card or a second accent colour. **Don't** enlarge the player at the list's expense.
- **Don't** show a position, track or state the daemon has not reported.
