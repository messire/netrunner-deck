# ADR 0004: deck-server panes are chats; each machine keeps its accounts

Status: accepted, 2026-11-24 (implemented: server and Deck)

Builds on [ADR 0003](0003-headless-claude.md) (Claude Code headless on the
stream-json control protocol) and [ADR 0002](0002-one-connection-type.md)
(one connection type: every server, local or remote, through its bridge).

## Context

After ADR 0003 a deck-server pane still began as a shell: the server made a
default workspace with one, and Deck turned a tab into a chat by typing
`deck-server agent` into it once a prompt showed (new tab, Quickhacks,
archive restore). That depended on shells, prompts, `PATH` and timing, and
a pane opened any other way (Herdr's API, a repository or worktree opened
on the server) stayed a console. The host reported its Claude session id
under a hook source Herdr did not keep, so the tree had no
`agent_session` for headless panes and ARCHIVE could not see them.

Accounts were switched by Deck running the local `cswap`, which only knows
the machine Deck runs on; a remote machine had no accounts at all and a
fresh one could not sign in without a console.

## Decision

### Every deck-server pane is a headless agent

deck-server spawns every pane as its own executable with `agent`
(`std::env::current_exe()`, never `PATH`), no shell, nothing typed
(`pane.rs` `set_agent_pane_program`, set by `deck-server` at start and on
a live handoff). That covers the default workspace, `workspace.create`,
`tab.create`, `pane.split`, repository and worktree opens, and restore.

* Restore after a restart starts the pane as `agent --resume <id>`
  (`agent_resume::agent_pane_resume_args`; a saved `claude --resume <id>`
  of an older server is mapped too). Nothing is typed.
* The host reports `agent_session_id` and `resume_argv` only once the
  session has had a turn (before that nothing exists on disk to resume).
  `deck:agent` / `deck-claude` is kept as a session owner like Herdr's
  native agents, so the tree's `agent_session` carries the Claude session
  id, it is saved in the session snapshot, and ARCHIVE lists and restores
  headless sessions.
* Two new host operations: `deck.agent.resume {pane_id, session_id}` makes
  the pane's host stop `claude` and start it again on that conversation
  (ARCHIVE's restore opens a tab, which is already an agent, and resumes in
  it); `deck.agent.restart {pane_id}` starts `claude` again on the same
  conversation once it is idle.
* The host's `running` note carries the session id, and the latest one is
  kept in the replay buffer, so a client knows the conversation (and shows
  its transcript) before `claude` says anything.

Deck has no typing path for deck-server any more: a new tab, a repository
or worktree opened from Deck, Quickhacks (`deck.agent.send` once the host is
up) and archive restore (`deck.agent.resume`) only select the pane. The new
tab picker and typed commands stay for a stock Herdr, whose panes are
shells.

### Accounts belong to the server's machine

deck-server keeps the Claude Code accounts of its machine
(`server/crates/deck-accounts`, served as `deck.accounts.*` by
`server/src/api/accounts.rs`); claude-swap's model, ported to Rust:

* store: `<app config dir>/accounts/accounts.json` and one
  `slot-N.json` per account (its `claudeAiOauth` credentials and its
  `oauthAccount` identity, owner-only on Unix);
* the live login is Claude Code's own: `.credentials.json` in
  `CLAUDE_CONFIG_DIR` or `~/.claude`, `oauthAccount` in `.claude.json`.
  `switch` writes the chosen slot there (other keys, e.g. MCP tokens, are
  kept) after saving the live, possibly refreshed, tokens of the account it
  leaves. A login made by other means is adopted into the store. Headless
  agents therefore run under the machine's active account; after a switch
  or sign-in the server asks every agent to restart `claude` at its next
  idle moment, so it picks the new login up;
* usage: Anthropic's usage endpoint per account (5-hour, 7-day,
  per-model windows), cached three minutes; an inactive account's expired
  token is refreshed in its slot, the active one is left to Claude Code;
* methods: `list` (shape of `cswap list --json`, plus `managed`,
  `signedIn`), `switch {number}`, `remove {number}` (not the active one),
  `login.start`, `login.finish {login_id, code, activate?}`.

HTTPS goes through the system's `curl` (Linux; `System32\curl.exe` on
Windows) with the request on its stdin (`-K -`), so deck-server stays a
static binary with no TLS stack and no secret appears in a process list.

### Sign-in without a console

The installed Claude Code signs in with OAuth + PKCE against its own client
(`claude.com/cai/oauth/authorize`, token endpoint
`platform.claude.com/v1/oauth/token`) and, when no browser can call it
back, the "manual" redirect `platform.claude.com/oauth/code/callback`,
whose page shows `CODE#STATE` to paste. deck-server does exactly that:
`login.start` makes the verifier, challenge and state and returns the URL;
Deck opens it in the user's browser and shows a field for the code;
`login.finish` exchanges it, reads the profile (email, organization,
subscription) and stores the account, making it the machine's login when
it has none (or when asked). `claude auth login` / `setup-token` were not
used: they need a terminal for the pasted code, and `setup-token` gives an
inference-only token with no usage. Credentials are issued on and stay on
the machine that signed in; nothing is copied between machines.

### Deck

ACCOUNTS, the account badge, F1 and the palette show the accounts of the
server that owns the selected session (else the server new spaces go to),
fetched over that server's connection; switching, adding (`+`, the same
card) and removing go to that server. A stock Herdr shows "accounts are
not managed on this server". Deck no longer runs `cswap`.

A chat shows the sign-in card when its machine has no login
(`signedIn: false`) or Claude Code says so (the `initialize` answer's
`account.tokenSource` is `none`, or a reply fails with
`authentication_failed`).

## Consequences

* A deck-server pane can only be a Claude chat; there is no shell on
  deck-server (stock Herdr keeps shells and the TERM view).
* The OAuth client id, endpoints and scopes are Claude Code's, read from
  the installed CLI; if Claude Code changes them, `deck-accounts` must
  follow (its tests pin the URL shape).
* Accounts of a machine are only as safe as its user's home directory: the
  store sits next to Claude Code's own credentials file with the same
  permissions.
* An agent restarts `claude` after an account change only when idle; a
  running turn finishes under the old login.
