# Changelog

Versions follow the rule in [README → Versioning](README.md#versioning): patch for fixes only, minor for new features (and, while 0.x, breaking changes), major from 1.0.0 for breaking changes.

## [0.4.0] — 2026-11-24
- Several servers in one Deck: SERVERS preferences with discovery, connect/disconnect, rename; panes of all servers in one tree.
- Remote servers over SSH: type `user@host` in SERVERS to add a machine. Every server, local or remote, is reached the same way, through a bridge on its machine (inside Deck for this one, `deck-server bridge` over ssh for another); addresses read `machine:socket`.
- Chat view for remote panes: deck-server serves Claude Code transcripts (`deck.transcript.read` / `deck.transcript.stat`).
- deck-server lives in this repo (`server/`), builds for Linux in CI and updates itself from the `deck-server-latest` release with a live handoff.
- Fixes: the unsent chat draft is kept per tab; crisp tool-step markers; chamfered frames keep their edge.

## [0.3.0] — 2026-11-20
- Chat mode built from the Claude Code transcript: dialogue as chat bubbles, browser-style text selection.
- Session archive: close a space to cold storage, restore with `claude --resume`, delete with its worktree; tree by repo › worktree › session.
- Harness, Quickhacks and Uplink panels.
- Sessions and Archive trees restyled; no blinking while the agent works.

## [0.2.0] — 2026-11-04
- Incremental pane rendering without full repaint; cursor from the fork's `pane.read`.
- Tree actions and hotkeys: context menus, `+` buttons, F1 account picker, F2 next waiting agent, Ctrl+K palette.
- One sidebar tree: repo › space › tab › session, pane split, WAITING strip.
- OS notification when an agent starts waiting; window state and selected pane remembered.
- Release by tag with signed installer and in-app updater; bundled Rajdhani font.
- Input box pinned below the chat; Shift+Enter, clipboard paste.

## [0.1.0] — 2026-10-24
- Tauri 2 + Solid + xterm.js client for the Herdr JSON API.
- Async commands with deadlines, OFFLINE banner, visible errors.
- Event routing with debounced tree refresh; output events from the fork.
- Terminal input mapped to Herdr key names; dialogue frames for Claude Code.
- CI: typecheck, vitest, cargo test, clippy, Windows build; CSP.
