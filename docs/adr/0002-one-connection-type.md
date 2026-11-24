# ADR 0002: One connection type for local and remote servers

Status: accepted, 2026-11-24

## Decision

Every server Deck talks to, on this machine or another one, is reached the
same way: a byte stream that carries Herdr's JSON-lines API unchanged (one
request per connection, `events.subscribe` keeps its connection). The
address decides only how that stream is opened (`src-tauri/src/endpoint.rs`):

| address | stream |
|---|---|
| `C:\…\herdr.sock`, `\\.\pipe\…`, `/…/herdr.sock` | the local named pipe / unix socket |
| `ssh://[user@]host[:port]/abs/path/herdr.sock` | a channel of `ssh host deck-server bridge` |

Above `endpoint.rs` nothing knows the difference: the same `HerdrRuntime`,
connection slots, deadlines, event pump, server registry and SERVERS row.

## Remote: the bridge

`deck-server bridge` (`server/src/deck_server/bridge.rs`) multiplexes any
number of connections to local API sockets over its stdin/stdout. Deck keeps
one `ssh … deck-server bridge` per host; a connection is an `open` frame plus
the request, sent at once, so one request costs one network round trip. The
frames only carry the server's own protocol, which stays Herdr's.

A host without the bridge (only Herdr, or an older deck-server) falls back to
`ssh -N -L 127.0.0.1:<port>:<remote socket>` per address: same stream, an SSH
channel per connection, a few round trips per request.

SSH itself is the user's OpenSSH: keys, agent, `~/.ssh/config`,
`known_hosts` (first-seen keys accepted, changed keys refused). `BatchMode`
is on: Deck never prompts.

## Transcripts

The chat view reads a Claude Code transcript where the agent runs. deck-server
answers `deck.transcript.read` / `deck.transcript.stat` on its API (handled
before Herdr's typed parse, so Herdr's schema is untouched). Deck asks the
pane's server first; only a local server without them (stock Herdr) falls back
to reading `~/.claude/projects` itself.

## Addresses stay local

Host names, users and addresses live only in Deck's `preferences.json` in its
local data dir, never in the repository. Typing `user@host` lists the servers
on that host over SSH, connects the preferred one (deck-server, else Herdr)
and adds the host to discovery.

## Not yet remote

Archive, harness and uplink still read this machine's files; for a remote
server they show only what is local.
