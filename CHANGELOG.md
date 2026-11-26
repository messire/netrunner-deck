# Changelog

Versions follow the rule in [README → Versioning](README.md#versioning): patch for fixes only, minor for new features (and, while 0.x, breaking changes), major from 1.0.0 for breaking changes.

## Unreleased

## [0.11.0] — 2026-11-27
- HARNESS, UPLINK, ARCHIVE and QUICKHACKS follow the server picked on the deck tab: what they show comes from that server's machine (its `~/.claude`, its checkouts, its closed sessions), labelled DECK or HERDR. With ALL, each server is its own section that folds like the server sections of SESSIONS. Pulls and pinned hacks are kept per server; RESTORE, DELETE and WRITE act on the entry's own server.
- QUICKHACKS: right-click a hack → COPY TO / MOVE TO another server. A hack whose skill or command that server's harness does not have is marked NOT IN THIS HARNESS.
- A view whose server cannot answer shows why (an older deck-server says to update it) instead of SCANNING… forever.
- SERVERS shows a non-default deck-server session's name next to the server (`deck@localhost · try`), so two sessions on one machine never look alike.
- PREFERENCES › CHAT: default permission mode and model for every new chat (CLI DEFAULT leaves them to Claude Code).
- PREFERENCES › NOTIFICATIONS: a Windows notification when an agent on any server needs you or finishes its turn, unless you are looking at that session; Herdr's sounds, each switch on its own. Coming back to Deck right after opens that session.
- Chats keep their conversation after the page reloads (they showed "new session — nothing said yet").
- Tidier chat: no blank lines inside lists, inline `code` shows as a chip while you type too, rounded image thumbnails, no `[Image #N]` chip in chat-only sessions.
- Deck looks for a new release every 10 minutes and when its window comes back to the front.

## [0.10.1] — 2026-11-25
- SECRETS lists every machine with a connected server, remote ones too (it showed only this PC), as tabs at the top; it opens on the selected session's machine. CONNECTIONS lists the same machines.
- SECRETS: COPY TO ▾ merges the shown `.env` into another machine's by key (same keys replaced, new keys appended, the rest kept) after a confirmation "N new, M replaced on <machine>".
- PREFERENCES keeps one size for every section; the SECRETS list has room below its last row.
- Esc, ✕ or a click outside no longer drop unsaved SECRETS edits: the panel stays and asks to SAVE or RELOAD.

## [0.10.0] — 2026-11-25
- PREFERENCES › SECRETS: `~/.claude/.env` of each machine (its deck-server) as a KEY / VALUE table — values masked with a reveal button per row, edit, + ADD, delete, SAVE, RELOAD. Comments, blank lines, order, `export`, quoting and CRLF are kept; Deck holds the values only while the panel is open. The server writes only that one file (atomically, 0600 on Unix).
- Installer with three kinds: client and server (default), client only, or server only. deck-server is installed windowless, starts when you sign in and updates itself; the choice is remembered and kept by updates. See docs/adr/0006-installer-kinds.md.
- deck-server on Windows never opens a console window (typed in a terminal it still prints there).
- COPY TO / MOVE TO between any connected servers, Herdr or deck-server, either way. The files are the work of Deck's bridge on each machine (built into Deck here, `deck-server bridge` there), so a machine with only Herdr works too and Herdr is not changed. On Herdr a branch arrives as a worktree in Herdr's own worktrees folder (it shows in its "open worktree"), joining the same repository when it is already there, and the session opens in a tab with `claude --resume`. Between two servers on one machine nothing is copied: the other server opens the same checkouts.
- Clone and GitHub sign-in are the bridge's work too, so they work on any machine Deck reaches.
- SESSIONS: a server's section folds and unfolds with a click on its header (▾/▸); the choice is remembered per server.

## [0.9.0] — 2026-11-25
- The chat header is taller; its labels and the mode and model buttons are 1.3× taller and sit on one centre line.
- Fix: a chat no longer blinks (the history vanishing for a moment, only the last reply left, then coming back). Showing a session again, or its id dropping for a moment while the live link to the server reconnects, now keeps the chat already read instead of emptying it and reading the transcript anew; the reply being written is not shown on its own before the history has loaded.
- deck-server on Windows updates itself like on Linux: CI publishes a Windows build to `deck-server-latest`; a running server downloads it (checksum verified), moves its own binary aside, puts the new one in place and restarts without a window, its sessions coming back from the snapshot.
- SERVERS shows each deck-server's build commit and marks one built by hand (or too old to say) as "LOCAL BUILD · NO UPDATES".
- The bottom bar shows this build (Deck version and commit, `dev` for a dev build) and the version of the selected session's server.

## [0.8.0] — 2026-11-25
- Clone from OPEN REPOSITORY: paste a git URL (https, ssh, `git@host:…`) and the server of that machine clones it into `~/{name}` (editable; a new or empty folder) with its own git and credentials, showing the phase and percent, then opens it as a new space. The clone goes on if the dialog is closed.
- GitHub sign-in per machine with the official GitHub CLI: when a github.com clone is refused, the dialog offers "Sign in to GitHub on {machine}". The server gets `gh` if the machine has none (official release, checksum verified, in its own data dir), runs `gh auth login --web`; Deck shows the one-time code and opens the link, you approve as your GitHub user, `gh` becomes git's credential helper there and the clone retries by itself. Claude's own git commands on that machine work too. PREFERENCES › CONNECTIONS shows per machine who it is signed in to GitHub as, with SIGN IN / SIGN OUT.
- Claude in a chat never waits for a git login prompt (`GIT_TERMINAL_PROMPT=0`): git fails at once with its reason.
- COPY TO / MOVE TO another server: right-click a repository (everything or chosen branches and sessions), a branch (it and all or some of its sessions) or a session. The repository arrives in `~/{name}` there with its full history, the chosen worktrees, uncommitted and untracked changes and its own harness; the Claude sessions arrive with their paths rewritten and resume there. MOVE removes them from the source only after the target has them. Busy sessions refuse; progress is shown and the work goes on when the dialog closes.
- HARNESS: PERSONAL and its items have COPY TO / MOVE TO as well (everything or chosen items; overwrite, skip or keep both on a name clash). Only instructions, skills, agents, commands and rules travel — never credentials, accounts, settings or transcripts.
- Fix: OPEN REPOSITORY on another machine said "not a directory" after CREATE · OPEN (Deck checked the folder on its own disk instead of leaving it to the server); quickhack launch and session restore no longer look at this PC's disk for a server's folders either.
- See docs/adr/0005-clone-and-transfer.md.

## [0.7.0] — 2026-11-25
- A new deck-server tab opens as a chat at once (it showed the agent's terminal until the tree caught up).
- Permission modes: bypass can be switched on; a mode or model change Claude refuses shows in the chat; modes are no longer checked against a built-in list, and after a server update a tab's agent replaces itself with the new binary on the same conversation.
- OPEN REPOSITORY on a server machine: lists the folders Claude Code knows there (not this PC's); a typed folder is always inside the server user's home (`repos`, `/repos`, `~/repos` → `<home>/repos`), the resolved path shows as you type, a missing folder is offered to be created ("Create folder …? CREATE · OPEN"); errors stay in the dialog.
- An empty chat says which machine and folder Claude works in. Errors name the server instead of "herdr".

## [0.6.0] — 2026-11-24
- deck-server: every pane is a Claude chat from the start. The server spawns each pane (default space, new space, tab, split, opened repository or worktree, restore after a restart) as its own `deck-server agent`, with no shell and nothing typed; Deck's typing path for deck-server is gone. See docs/adr/0004-chat-only-panes-and-accounts.md.
- ARCHIVE sees headless sessions: their Claude session id is in the tree (`agent_session`) and in the server's session snapshot; restore opens a tab and resumes the conversation in it (`deck.agent.resume`).
- Accounts per machine: deck-server keeps its machine's Claude accounts (list with 5h/7d usage, switch, remove; claude-swap's model in Rust) as `deck.accounts.*`. ACCOUNTS, the account badge and F1 show the accounts of the server that owns the selected session; a stock Herdr says accounts are not managed there. Deck no longer runs `cswap`.
- Sign in without a console: a chat on a machine with no Claude login shows a sign-in card; the server starts Claude's OAuth sign-in, Deck opens the link, you paste the code, the server keeps the account. ACCOUNTS' `+` adds another account the same way. After a switch or sign-in the machine's agents restart Claude when idle.
- The chat's mode button offers every Claude Code permission mode: manual, accept edits, plan, auto, don't ask, bypass.
- Fixes: the bridge no longer keeps a socket for every answered one-shot call (a long-lived remote bridge ran out of file descriptors, "os error 24", and its server showed OFFLINE); deck-server raises its open-files limit at start. The sign-in card keeps its code field while you sign in, and goes away once Claude runs signed in. Tidier chat header, narrower session tabs, no help text under SERVERS' + ADD.

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
