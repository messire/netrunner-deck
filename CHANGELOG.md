# Changelog

Versions follow the rule in [README → Versioning](README.md#versioning): patch for fixes only, minor for new features (and, while 0.x, breaking changes), major from 1.0.0 for breaking changes.

## [Unreleased]
- deck-server: every pane is a Claude chat from the start. The server spawns each pane (default space, new space, tab, split, opened repository or worktree, restore after a restart) as its own `deck-server agent`, with no shell and nothing typed; Deck's typing path for deck-server is gone. See docs/adr/0004-chat-only-panes-and-accounts.md.
- ARCHIVE sees headless sessions: their Claude session id is in the tree (`agent_session`) and in the server's session snapshot; restore opens a tab and resumes the conversation in it (`deck.agent.resume`).
- Accounts per machine: deck-server keeps its machine's Claude accounts (list with 5h/7d usage, switch, remove; claude-swap's model in Rust) as `deck.accounts.*`. ACCOUNTS, the account badge and F1 show the accounts of the server that owns the selected session; a stock Herdr says accounts are not managed there. Deck no longer runs `cswap`.
- Sign in without a console: a chat on a machine with no Claude login shows a sign-in card; the server starts Claude's OAuth sign-in, Deck opens the link, you paste the code, the server keeps the account. ACCOUNTS' `+` adds another account the same way. After a switch or sign-in the machine's agents restart Claude when idle.

## [0.5.0] — 2026-11-24
- SERVERS: + ADD opens a dialog (user, host, port) whose CHECK lists the machine's servers and pings each through its bridge; only the ones that answer are offered, with kind and version, to connect in one go. Non-default SSH ports work.
- SERVERS: ✕ removes a machine with all its servers (disconnects them, closes its bridge, stops scanning it); manually added addresses can be removed too.
- The periodic scan pings remote servers through the bridge and offers only those that answer; a machine that cannot be reached shows as one grey row with the reason instead of its old rows.
- deck-server runs Claude Code headless: `deck-server agent` in a pane starts Claude on the stream-json control protocol, without a terminal, and serves it as `deck.agent.*` (subscribe with replay, send, permission answers, interrupt, model and mode). See docs/adr/0003-headless-claude.md.
- Deck shows a Claude tab on deck-server only as a chat, with no console: new tabs, Quickhacks and archive restore start `deck-server agent`; the reply streams in as it is written; permission requests, AskUserQuestion and plan approval are cards in the chat; permission mode, model and stop are in the header; pasted images go with the message.

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
