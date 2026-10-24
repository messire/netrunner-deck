# Netrunner Deck

Desktop GUI client for [Herdr](https://github.com/herdrdev/herdr) in the Daemon 2.0 / Cyberpunk
look. It is a *client*: every workspace, tab, pane and agent you see here lives in the Herdr
server, exactly as in the Herdr TUI. Deck keeps no session state of its own — the tree is
re-fetched from Herdr, pane contents are `pane.read` snapshots, account data comes from `cswap`
on every call. (UI-only settings such as window size may be persisted from milestone M4 on;
none are today.)

Stack: Tauri 2 (Rust) + Solid + xterm.js.

## Requirements

- A running Herdr server. Deck talks to its JSON API endpoint: `<config dir>/herdr.sock`
  (unix socket; on Windows the same path as a named pipe under `\\.\pipe\`). See
  "Environment variables" for overrides.
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
| `npm test` | `vitest run` — `src/**/*.test.ts` in a Node environment (dialogue heuristics, key mapping, event routing, latest-wins gate) |
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
- **Pane view = snapshots + heuristics.** Herdr emits no output event, so `TerminalView` polls
  `pane.read` every 2.5 s and shortly after your own input, then repaints the xterm from
  scratch. Dialogue frames (user turn / agent reply) are xterm decorations placed by
  heuristics over Claude Code's screen layout (`src/dialogue.ts`); the cursor is put at the
  end of the prompt line as an estimate — the snapshot has no cursor position. While you
  scroll back or hold a selection, new snapshots are parked and a `NEW OUTPUT ↓` hint appears
  in the pane header.
- **Keys.** xterm input is mapped to Herdr key names (`src/keys.ts`): printable text →
  `pane.send_text`, control bytes and escape sequences → `pane.send_keys`. Herdr wraps
  `send_text` in bracketed paste, so unknown escape sequences are dropped rather than pasted.
- **Accounts.** The sidebar shows `cswap list --json`; double-click on an account runs
  `cswap switch <n> --json` (one switch at a time; errors are shown in the card).

Not there yet:

- **Streaming attach.** No live stream of pane output; the view is snapshot-based (milestone M2).
- **Home / End / Insert / Delete / PageUp / PageDown.** Herdr's `send_keys` has no names for them,
  so Deck drops these keys until the fork's key grammar learns them (milestone M1.5).
- **Hotkeys and tree actions** (account picker, jump to waiting agent, command palette): milestone M3.

## Layout

- `src/` — Solid frontend: `App.tsx` (state, event routing), `components/Sidebar.tsx`
  (spaces, accounts, agents, session map), `components/TabBar.tsx`,
  `components/TerminalView.tsx` (xterm pane view), `components/KeyBar.tsx`;
  `dialogue.ts`, `keys.ts`, `events.ts`, `latest.ts` are pure modules with tests next to them;
  `__fixtures__/pane-read/` holds synthetic Claude Code snapshots for the dialogue tests.
- `src-tauri/src/herdr.rs` — Herdr JSON API client (named pipe on Windows, unix socket elsewhere),
  deadline and connection slots, event subscription.
- `src-tauri/src/lib.rs` — Tauri commands, `cswap` resolution, the event pump.
- `scripts/fake-herdr.mjs` — fake Herdr server for manual bridge tests.
- `docs/architecture.md` — data flow, verified Herdr API facts (F1–F10), module map.

## Roadmap

See `docs/plan/ROADMAP.md` (milestones M1a/M1b → M1.5 → M2 streaming → M3 actions and hotkeys
→ M4 polish → M5 release).
