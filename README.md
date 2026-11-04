# Netrunner Deck

Desktop GUI client for [Herdr](https://github.com/herdrdev/herdr) in the Daemon 2.0 / Cyberpunk
look. It is a *client*: every workspace, tab, pane and agent you see here lives in the Herdr
server, exactly as in the Herdr TUI. Deck keeps no session state of its own — the tree is
re-fetched from Herdr, pane contents are `pane.read` snapshots, account data comes from `cswap`
on every call. (UI-only settings such as window size may be persisted from milestone M4 on;
none are today.)

Stack: Tauri 2 (Rust) + Solid + xterm.js. UI font: Rajdhani (Indian Type Foundry, SIL OFL 1.1),
bundled from `src/fonts/` with its `OFL.txt`; nothing is fetched at runtime.

Deck keeps two UI-only preferences on your machine: the window size/position
(`tauri-plugin-window-state`) and the last selected pane (webview `localStorage`). Nothing from
Herdr is stored.

## Requirements

- A running Herdr server. Deck talks to its JSON API endpoint: `<config dir>/herdr.sock`
  (unix socket; on Windows the same path as a named pipe under `\\.\pipe\`). See
  "Environment variables" for overrides. On Windows a Herdr started from an administrator
  terminal is reachable only by elevated clients: either run Deck as administrator or restart
  Herdr with `herdr server --allow-unelevated-clients` (the OFFLINE banner says which).
- `cswap` for the Accounts section. It is resolved once at startup from `PATH`, then
  `~/.local/bin` (`%USERPROFILE%\.local\bin` on Windows, trying the `PATHEXT` extensions, then
  `cswap.exe`, `cswap.cmd`, `cswap`). If it is not found the Accounts block shows
  `cswap not found (searched PATH and ~/.local/bin)` and the rest of the UI works.
- Rust 1.99.1 — pinned in `rust-toolchain.toml` (rustup picks it up automatically).
- Node 22 (what CI uses).
- Windows build: Visual Studio C++ Build Tools + Windows SDK; WebView2 (ships with Windows 11).
  CI builds only the Windows target; the unix branches of `src-tauri/src/herdr.rs` are not
  compiled in CI.

## Dev

```powershell
npm install
npm run tauri dev
```

npm scripts (`package.json`):

| Script | What |
|---|---|
| `npm run dev` | Vite dev server only (no Tauri window) |
| `npm run typecheck` | `tsc --noEmit` |
| `npm test` | `vitest run` — `src/**/*.test.ts` in a Node environment (dialogue heuristics, key mapping, event routing, latest-wins gate, render plan, hotkeys/palette) |
| `npm run build` | `tsc --noEmit && vite build` |
| `npm run tauri dev` / `npm run tauri build` | Tauri app in dev mode / release bundle (NSIS) |

Rust side: `cargo test` and `cargo clippy --all-targets` with `--manifest-path src-tauri/Cargo.toml`
(both run in `.github/workflows/build.yml` before the Windows build).

### Fake Herdr for bridge tests

`scripts/fake-herdr.mjs` is a stand-in JSON API server for exercising the bridge without Herdr:

```
node scripts/fake-herdr.mjs <mode> [path]
# modes: silent | slow <ms> | ack-error | close-after-ack | ok
```

Point Deck at it with `HERDR_SOCKET_PATH`. On Windows the variable is a pipe *name*
(e.g. `deck-fake`; the bridge prefixes `\\.\pipe\` itself); elsewhere it is a socket path
(default `/tmp/deck-fake.sock`). `silent` never replies (exercises the 15 s deadline),
`ack-error` refuses every request, `close-after-ack` drops the connection after the first reply.

## Environment variables

| Variable | Effect | Default |
|---|---|---|
| `HERDR_SOCKET_PATH` | Explicit Herdr endpoint (socket path; pipe name on Windows) | `<config dir>/herdr.sock` |
| `HERDR_CONFIG_DIR` | Herdr config directory used to derive the endpoint | `%APPDATA%\herdr` on Windows; `$XDG_CONFIG_HOME/herdr` or `~/.config/herdr` elsewhere |
| `RUST_LOG` | `tracing` filter for the Rust side | `info` |

## How it works

- **Thin client.** Every Tauri command (`src-tauri/src/lib.rs`) is one or more request/reply
  exchanges with Herdr (`src-tauri/src/herdr.rs`): `herdr_tree` = `workspace.list` +
  `tab.list` per workspace + `pane.list` + `agent.list`; `herdr_pane_read` = `pane.read`
  (`source: recent`, `format: ansi`); input goes through `pane.send_text` / `pane.send_keys`;
  `herdr_focus` maps to `workspace.focus` / `tab.focus` / `pane.focus`.
- **No UI freeze.** Commands are async and run on the blocking pool; each call has a 15 s
  deadline and at most 2 request/reply connections are open at once (Windows named pipes
  answer `ERROR_PIPE_BUSY` otherwise). Timeouts surface in the UI as errors.
- **Events.** A background thread holds one `events.subscribe` connection and forwards every
  event to the webview as `herdr://event`; a broken or refused subscription becomes
  `herdr://offline` (the OFFLINE banner) and is retried every 2 s. All events are tree
  metadata; a burst becomes one `herdr_tree` refetch 200 ms after the burst ends
  (`src/events.ts`). The tree and accounts are also re-fetched every 15 s.
- **Pane view = snapshots + heuristics.** `TerminalView` reads `pane.read` on the fork's
  `pane_output_changed` event (at once, then at most every 100 ms while output streams),
  shortly after your own input, and on a slow safety poll (2.5 s on upstream Herdr, which has
  no output event; 10 s once events are seen), then applies the snapshot incrementally. Dialogue frames (user turn / agent reply) are xterm decorations placed by
  heuristics over Claude Code's screen layout (`src/dialogue.ts`); the cursor is put at the
  end of the prompt line as an estimate — the snapshot has no cursor position. While you
  scroll back or hold a selection, new snapshots are parked and a `NEW OUTPUT ↓` hint appears
  in the pane header.
- **Keys.** xterm input is mapped to Herdr key names (`src/keys.ts`): printable text →
  `pane.send_text`, control bytes and escape sequences → `pane.send_keys`. Herdr wraps
  `send_text` in bracketed paste, so unknown escape sequences are dropped rather than pasted.
  `Shift+Enter` is sent by name (a newline in Claude Code); `Ctrl+V` / `Shift+Insert` paste the
  clipboard text (read on the Rust side) as one bracketed paste.
- **Accounts.** The sidebar shows `cswap list --json`; double-click on an account runs
  `cswap switch <n> --json` (one switch at a time; errors are shown in the card).
- **Incremental rendering (M2).** A new snapshot is aligned with what is already in the xterm
  buffer (`src/render-plan.ts`) and only the changed tail is rewritten, so scrollback, scroll
  position and selection survive updates; a full repaint happens only on first show, pane switch
  and resize. On the fork, `pane.read` also returns the cursor (`cursor.line`/`col`), which Deck
  places in the input box; on upstream Herdr the cursor is estimated at the end of the prompt line.
- **One tree (M6).** The sidebar is a single tree — repo (yellow) › space / worktree (red) ›
  tab-session (cyan) — instead of a Spaces list plus a status-sorted Agents list. A tab with one
  pane is one row; a split tab keeps its row and lists its sessions one level deeper. Every level
  collapses (▾/▸; busy or waiting nodes and the one holding the selected pane start open, manual
  toggles are remembered) and every level has its own `+`: new space in the repo, new tab in the
  space, split next to a session. Blocked agents are pinned in a WAITING strip above the tree with
  their `repo / space / tab` path; `F2` cycles through them. Server order everywhere else.
- **Actions and hotkeys (M3).** Right-click a space, tab, session or account for its menu (focus in
  Herdr, new tab / new space, split right/down, close — destructive items ask for a second click);
  `+` next to SESSIONS and in the tab bar create a workspace / tab. `F1` account picker, `F2` jump to the next
  agent that waits for input, `Ctrl+K` command palette, `Esc` closes an overlay (and otherwise
  goes to the CLI). These keys never reach the pane. Every action is one Herdr method
  (`workspace.create/close`, `tab.create/close`, `pane.close`, `pane.split`, `*.focus`) behind the `herdr_action`
  allowlist; failures show in the title bar.
- **Notifications (M4).** When an agent turns `blocked` (waits for input) and its pane is not on
  screen, or the window is in the background, Deck raises an OS notification
  (`tauri-plugin-notification`); clicking the agent in the sidebar or `F2` brings its pane up.
- **Browser mode.** `npm run dev` and open `http://localhost:1420/` in a browser: without Tauri the
  bridge is replaced by an in-memory stand-in (`src/tauri.ts`) with synthetic workspaces and a
  transcript that keeps growing, so the UI can be looked at and driven without Herdr.

Not there yet:

- **Home / End / Insert / Delete / PageUp / PageDown** on upstream Herdr: its `send_keys` has no names
  for them, so Deck drops these keys there (the fork accepts them, M1.5).
- **Alternate screen.** A full-screen program in the pane (vim, less) is shown as plain text; the
  fork reports `alternate_screen`, but Deck has no dedicated mode for it yet.

## Layout

- `src/` — Solid frontend: `App.tsx` (state, event routing, menus, hotkeys), `components/Sidebar.tsx`
  (the repo › space › tab › session tree, accounts, session map; model in `tree.ts`), `components/TabBar.tsx`,
  `components/TerminalView.tsx` (xterm pane view), `components/KeyBar.tsx`,
  `components/ContextMenu.tsx`, `components/Palette.tsx`;
  `dialogue.ts`, `keys.ts`, `events.ts`, `latest.ts`, `render-plan.ts`, `actions.ts` are pure
  modules with tests next to them;
  `__fixtures__/pane-read/` holds synthetic Claude Code snapshots for the dialogue tests.
- `src-tauri/src/herdr.rs` — Herdr JSON API client (named pipe on Windows, unix socket elsewhere),
  deadline and connection slots, event subscription.
- `src-tauri/src/lib.rs` — Tauri commands, `cswap` resolution, the event pump.
- `scripts/fake-herdr.mjs` — fake Herdr server for manual bridge tests.
- `docs/architecture.md` — data flow, verified Herdr API facts (F1–F10), module map.

## Release

`package.json` holds the version; `tauri.conf.json` reads it (`"version": "../package.json"`) and
`npm run version:sync` copies it into `src-tauri/Cargo.toml` (+ `Cargo.lock`); CI fails on a
mismatch (`npm run version:check`). To release:

```powershell
npm version 0.2.0 --no-git-tag-version
npm run version:sync
git commit -am "release: v0.2.0" && git tag v0.2.0 && git push && git push --tags
```

`.github/workflows/release.yml` builds the Windows installer on the tag (same checks as
`build.yml`) and publishes a GitHub Release with the NSIS installer and the bare `.exe`; a tag with
a pre-release suffix (`v0.2.0-rc.1`) is marked pre-release. The build workflow stays read-only.

**Updates.** The installer is signed (`tauri-plugin-updater`, minisign) and the release carries
`latest.json`; Deck checks `releases/latest/download/latest.json` on start and every 4 h and, when a
newer version exists, shows `⇣ UPDATE vX.Y.Z — INSTALL` in the title bar. Nothing downloads or
installs until you click it; the NSIS installer then runs in passive mode and restarts the app.
Pre-releases are never offered (`latest` skips them). `tauri dev` does not check.

The private key lives outside the repository (generated with `npx tauri signer generate -w
~/.tauri/netrunner-deck.key`); the release workflow needs it as repository secrets
`TAURI_SIGNING_PRIVATE_KEY` (file contents) and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, and fails
early when the key is missing. Its public half is `plugins.updater.pubkey` in `tauri.conf.json`;
rotating the key means shipping one release signed with the old key that carries the new pubkey.

## Roadmap

See `docs/plan/ROADMAP.md` (milestones M1a/M1b → M1.5 → M2 streaming → M3 actions and hotkeys
→ M4 polish → M5 release).
