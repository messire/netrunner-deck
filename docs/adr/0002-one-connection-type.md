# ADR 0002: One connection mechanism for every server

Status: accepted, 2026-11-24

## Decision

Deck connects to every server, on this machine or another one, in exactly one
way: a channel of the `deck-server bridge` running on the server's machine.
There is no direct pipe/socket connection and no second transport.

* The bridge (`server/src/deck_server/bridge.rs`) carries any number of
  connections over its stdin/stdout. Each channel is a plain connection to an
  API socket on its machine (deck-server's or any Herdr's), so what flows in
  it is the server's own JSON-lines protocol, unchanged: Herdr-compatible.
* Deck (`src-tauri/src/endpoint.rs`, `bridge.rs`) keeps one bridge per
  machine. The address says which machine and which socket:

| address | bridge |
|---|---|
| `C:\…\herdr.sock` (shown as `\\.\pipe\…`), `/…/herdr.sock` | `deck-server bridge`, a child process here |
| `ssh://[user@]host[:port]/abs/path/herdr.sock` | `ssh host deck-server bridge` |

Above `endpoint.rs` nothing differs: the same runtime, connection slots,
deadlines, event pump, server registry, SERVERS row and chat view.

A channel is opened by sending its `open` frame and the request at once, so a
request costs one round trip to the bridge.

## Requirements

* deck-server on every machine with servers Deck uses. Deck finds the local
  one next to its own executable, then in `~/.local/bin`, then on PATH; on an
  SSH host, on PATH or in `~/.local/bin`.
* SSH is the user's own OpenSSH (keys, agent, `~/.ssh/config`,
  `known_hosts`; first-seen keys accepted, changed keys refused). Deck never
  prompts.

## Transcripts

The chat view reads a Claude Code transcript where the agent runs: deck-server
answers `deck.transcript.read` / `deck.transcript.stat` on its API (handled
before Herdr's typed parse, so Herdr's schema is untouched). Deck asks the
pane's server; only a local server without them (stock Herdr) falls back to
reading `~/.claude/projects` itself.

## Addresses stay local

Host names, users and addresses live only in Deck's `preferences.json` in its
local data dir, never in the repository. Typing `user@host` lists the servers
on that host over SSH, connects the preferred one (deck-server, else Herdr)
and adds the host to discovery.

## Not yet remote

Archive, harness and uplink still read this machine's files.
