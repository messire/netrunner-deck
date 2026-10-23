# Netrunner Deck

Desktop GUI client for [Herdr](https://github.com/herdrdev/herdr) in the Daemon 2.0 / Cyberpunk
look. It is a *client*: every workspace, tab, pane and agent you see here lives in the Herdr
server, exactly as in the Herdr TUI. Nothing is duplicated or stored by the app.

Stack: Tauri 2 (Rust) + Solid + xterm.js.

## Dev

```powershell
npm install
npm run tauri dev
```

Requirements: Rust (msvc), Node LTS, Visual Studio C++ Build Tools, WebView2 (ships with Windows 11),
and a running Herdr server (`herdr` in PATH; the app talks to its JSON API pipe).

## Layout

- `src/` — Solid frontend (sidebar tree, terminal view, session map, tab bar).
- `src-tauri/src/herdr.rs` — Herdr JSON API client (named pipe on Windows, unix socket elsewhere).
- `src-tauri/src/lib.rs` — Tauri commands and the event pump that forwards Herdr events to the UI.

## Roadmap

1. Tree (spaces / accounts / agents) + one pane view via `pane.read` snapshots.
2. Streaming pane attach (`pane.stream`, added in the herdr-punk fork) → real xterm.
3. Session map, dialogue framing inside the terminal, hotkeys, cswap switching.
