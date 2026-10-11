# Herdr Spotify development

- Personal plugin, two Macs, one Spotify Premium account. Local player, not a remote control.
- Read `ARCHITECTURE.md` before changing code: it maps the modules and gives the steps for adding a request, a piece of state, a Web API fetch, a screen, or a key. Extend those patterns instead of adding parallel ones.
- Gate: run `scripts/regress` after every change and before every commit or handoff; it must print `PASS`. Use `--quick` while iterating.
- Build with `--locked`. The lockfile pins `vergen` 9.0.6; after adding a dependency, confirm the pin is still there.
- Web API calls only when needed. Every request goes through `Api::get`; anything new states its budget in `src/web/catalog.rs` and gets a counting test. Never add polling, automatic retries, or a Web API call for player state.
- Never load anything into librespot before the user asks to play. Starting the daemon must not take playback from another device.
- Secrets: the two refresh tokens live only in the state directory files written by `src/credentials.rs`. Never print, log, snapshot or commit a token. The client ID is not a secret; a client secret does not exist and must not be added.
- Tests never touch the real state or config directories, the real Spotify, or the real sign-ins. Use the sandbox in `tests/common/`.
- UI snapshots in `tests/snapshots/ui/` specify every popup state. Re-record with `scripts/regress --update-snapshots` only for an intended UI change, review `git diff tests/snapshots`, and commit them with the change. Never re-record, loosen, or skip a check to pass.
- UI work goes through Impeccable with `--target` a file in this repo; `PRODUCT.md` and `DESIGN.md` are the context. The look inherits the Personal Inbox's.
- After touching `src/daemon/player.rs` or `src/daemon/playback.rs`, check by hand with real audio: play, pause, skip, stop the daemon, resume. Tell Allar before playing audio on his machine.
- Implement only explicitly requested changes. Ask Allar before adding features, options, or scope.
- Commits: use `the workspace committer script`.
- The design spec is local content in `a local folder outside this repository`. Never commit or upload it.
