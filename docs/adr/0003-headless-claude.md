# ADR 0003: Claude Code runs headless; Deck shows only the chat

Status: accepted, 2026-11-24 (implemented: server and Deck). Amended by
[ADR 0004](0004-chat-only-panes-and-accounts.md): every deck-server pane
now starts as the host directly (no shell, nothing typed), and sign-in and
accounts are the server's.

## Context

Deck showed a Claude pane as a chat only once deck-server reported the
session id, and read the conversation from the JSONL transcript. Everything
the transcript does not carry was only on the terminal: first-run
onboarding (theme, login), permission prompts, AskUserQuestion, plan
approval, slash-command menus. On a new remote machine the user saw Claude
Code's console. The user must never see a console in Deck, only the chat.

Hiding the terminal and reading its screen would keep all of that on
heuristics over Claude Code's layout, which change with every release.

## Decision

Claude Code runs without a terminal, on the stream-json control protocol
(the one the Agent SDK speaks), spoken directly by deck-server in Rust:

    claude -p --input-format stream-json --output-format stream-json --verbose
           --include-partial-messages --replay-user-messages
           --permission-prompt-tool stdio (--session-id ID | --resume ID)

### Host in the pane

`deck-server agent [--resume ID] [--model M] [--permission-mode M]` is what
every deck-server pane runs (ADR 0004; first it was typed into the pane's
shell) (`server/src/deck_server/agent_host.rs`).
It starts `claude` with pipes and relays it:

* every output line goes to the server as a numbered event
  (`{"seq":N,"msg":{..}}`, numbered per host process, its `epoch`);
* what Deck asks for comes back as an operation and is written to stdin:
  a message (text and images), the answer to a permission request
  (`{"behavior":"allow","updatedInput":..}` or `deny`; AskUserQuestion
  answers ride in `updatedInput.answers`), interrupt, model, permission mode.

The host adds its own events: `deck_resolved` (a request was answered, by
any client), `deck_settings` (claude confirmed a model/mode change),
`deck_host` (`running`, `exited`, `log` = a stderr line).

The host reports the pane as agent `deck-claude` under the hook source
`deck:agent` with state idle / working / blocked read from the stream and
`resume_argv = deck-server agent --resume <id>`. So the tree, the sidebar
status and Herdr's restore after a server restart work unchanged: restore
starts the new pane as `agent --resume <id>` (ADR 0004). Herdr's own Claude
hooks are switched off for this `claude` (no `HERDR_ENV`), so they do not
claim the pane. The server puts its own directory first on every pane's
`PATH`, so `deck-server` is a bare command there.

If `claude` exits, the host starts it again on the same session (a new
session id is only used until the first turn). If the server goes away
(restart, self-update with live handoff) the host reconnects and replays
its buffer; the server skips events it already has.

Living in the pane keeps the session's lifetime, cwd, env and restore
exactly those of the pane, with no change to Herdr's pane model.

### Server API (`server/src/api/agent.rs`)

Deck extensions next to Herdr's API, answered before the typed parse like
`deck.transcript.*`; nothing in Herdr's schema changes.

* Host: `deck.agent.host` (host → server events) and `deck.agent.inbox`
  (server → host operations), one direction per connection, so Windows
  synchronous pipes never block a read against a write.
* Clients: `deck.agent.subscribe {pane_id, epoch?, after_seq?}` (replay,
  then live `deck.agent.event`; `deck.agent.reset` when a new host takes
  the pane), `deck.agent.send | respond | interrupt | set`, `deck.agent.list`.

Several clients can show the same pane: all get every event; the first
answer to a permission request wins, a second is `already_answered`.

The replay buffer (`server/crates/deck-agent`, shared by host and server)
keeps the current turn, the session's `system/init` and `initialize`
response, and every open permission request. At each `result` the rest is
dropped: the transcript has the full history.

## Consequences

* Deck opens a Claude tab on deck-server as `deck-server agent` (new tab,
  Quickhacks, archive restore) and shows it only as a chat: history from
  the transcript, the reply being written, and cards for permission
  requests (allow / always allow / deny with a note), AskUserQuestion and
  plan approval from `deck.agent.subscribe` (`src/agent.ts`,
  `src/agentStore.ts`, `src/components/AskCard.tsx`,
  `src-tauri/src/agent.rs`). The header has the session's permission mode,
  model and a stop button; the slash popup lists the session's own
  commands. A headless pane has no TERM view. Herdr panes keep theirs.
* Claude Code's interactive built-in commands (`/model`, `/config`, `/mcp`,
  `/resume`, `/rewind`, `/login`, `/doctor`) do not exist headless; Deck
  offers the needed ones itself. Custom commands and skills work as text.
* The control protocol is not a documented public format and can change
  with Claude Code; the fixture in `server/crates/deck-agent/fixtures` and
  its tests pin what the host relies on.
* A stock Herdr has no host: there Deck keeps today's terminal + transcript
  view.
* Login on a fresh machine and the accounts of each machine: ADR 0004.
