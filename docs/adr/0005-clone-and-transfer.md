# ADR 0005: Clone, GitHub sign-in and transfers are the server's work

Status: accepted, 2026-11-25 (implemented: server and Deck)

Builds on [ADR 0002](0002-one-connection-type.md) (one connection type) and
[ADR 0004](0004-chat-only-panes-and-accounts.md) (each machine keeps its own
accounts).

## Decision

Deck never touches a disk or runs git from its own code paths: every
clone, sign-in and transfer is work of the machine concerned, asked as a
`deck.*` method over the same single connection as everything else.

It is answered by **Deck's bridge on that machine** (ADR 0002): besides
server sockets, a bridge opens one more channel, `deck:machine`, which is
the bridge itself running `deck_accounts::machine` (`clone`, `github`,
`transfer`, `pack`, `jobs`, `proc` in `server/crates/deck-accounts`). The
bridge is there on every machine Deck reaches — built into Deck for its own
machine, `deck-server bridge` over ssh for another — so this works the
same whether the server there is deck-server or Herdr, and Herdr is never
asked or changed. Deck's `machine_call` sends these methods to
`Endpoint::machine_channel()` of the server's machine; deck-server also
answers them on its own API (and `deck.server.info` about itself).

Anything long is a **job**: `*.start` returns `{job}` at once, the work runs
on its own thread, `*.status {job}` returns `{state, phase, percent,
message, …}`. A job lives in the bridge (or server) process that started it
and outlives the dialog that started it; Deck's polling loops
(`src/jobs.ts`) are not tied to a component either. A job ends with its
bridge (Deck quitting, ssh dropping).

git runs without a window and never asks (`proc::git`):
`GIT_TERMINAL_PROMPT=0`, `GCM_INTERACTIVE=never`, ssh in `BatchMode` (the
user's own `GIT_SSH_COMMAND` / `core.sshCommand` is kept, only put in batch
mode; otherwise a first-seen host key is accepted, a changed one refused).
A refusal comes back as an error, marked `auth` when it was credentials.

### Clone

`deck.git.clone.start {url, path}` → `{job, path}`; `status`, `cancel`.
The target is a new or empty folder, resolved like OPEN REPOSITORY's
(`place_of`: inside the home on a server machine). Progress is
`git clone --progress` parsed into phase and percent. A failed or cancelled
clone leaves nothing behind.

### GitHub sign-in

The official GitHub CLI, run by the server (`deck_accounts::github`): no
app registration, no client id, no token typed anywhere.

* `gh` is the machine's own when it is on `PATH` (or `~/.local/bin`);
  otherwise the server downloads the latest official release for its OS
  and architecture from GitHub, checks it against the release's
  checksums and keeps it in `<config dir>/tools/gh/bin` (no root, no
  package manager).
* `deck.github.login.start` → `{login_id}` starts `gh auth login --hostname
  github.com --git-protocol https --web` without a terminal (`GH_BROWSER`
  a no-op); `deck.github.login.status` → `{state: installing | starting |
  pending | done | failed, user_code, verification_uri, user, message}`.
  Deck shows the code and opens the link in the user's browser; the user
  approves as whichever GitHub user they are. Then `gh auth setup-git`
  makes `gh` git's credential helper for github.com on that machine, so a
  clone from Deck, Claude's Bash and a terminal there all reach the
  user's repositories.
* `deck.github.status` → `{available, signed_in, user}`, `deck.github.logout`.
  PREFERENCES › CONNECTIONS shows this per connected deck-server.
* The token is `gh`'s, where `gh` keeps it; Deck never sees it.

A clone of a github.com address given over SSH that is refused is tried
once more over HTTPS. Headless agents run `claude` with
`GIT_TERMINAL_PROMPT=0`, so git in a chat fails instead of waiting for a
login nobody can type.

### Transfer (COPY TO / MOVE TO)

The source packs (`deck.transfer.export.start`), Deck relays the pack in
512 KiB chunks from `export.read` to the target's `import.write` on a
thread of its own (`transfer_relay`; base64 passed through, never decoded,
never on Deck's disk), the target unpacks (`import.commit`). A move then
asks the source to remove what arrived (`remove.start`) — only after the
target reported success, and after Deck closed the moved sessions and
worktree spaces there.

A pack (`pack.rs`) is a flat list of named byte runs with checked relative
names. A repository pack carries a git bundle (every ref for the whole
repository, else the chosen branches and the tags), the remotes, per
chosen worktree `git diff --binary HEAD` and the untracked, not ignored
files (plus the repository's own `.claude/`, `CLAUDE.md`,
`CLAUDE.local.md` even where ignored), and the chosen Claude sessions'
transcripts with their sidecar folders. On the target the repository goes
to `~/{name}` (refused when taken; Deck asks for another name), linked
worktrees to `worktrees.directory/{name}/{branch}`, transcripts to
`~/.claude/projects/<new folder>` with the old paths in them rewritten.
Deck then opens the worktrees as spaces and resumes the sessions there
(`transfer_open`, `deck.agent.resume`).

A busy session (working, or waiting for an answer) refuses the transfer.

Any server to any server. The target's kind decides where new worktrees go
(`import.start {for}`): Herdr's or deck-server's `[worktrees] directory`
from its `config.toml`, default `~/.herdr/worktrees/{repo}/{branch}`, so
Herdr lists them in its own "open worktree". When the same repository is
already at `~/{name}` (a shared root commit or remote), the sent branches
join it as worktrees (a branch already there at the same commit is used
where it is checked out; one with other commits is refused) and nothing
there is overwritten. Deck then opens the worktrees with the target's own
`worktree.open` and resumes each session: `deck.agent.resume` on
deck-server, a tab with `claude --resume <id>` typed on Herdr. Between two
servers on the same machine nothing moves: the target opens the same
checkouts and transcripts, and a move only closes them on the source.

A harness pack carries items of the personal `~/.claude` only
(`CLAUDE.md`, `skills/`, `agents/`, `commands/`, `rules/`,
`output-styles/`); the server refuses anything else, so credentials,
accounts, settings and transcripts never travel. Items that exist on the
target: overwrite, skip or keep both (`name-2`).

## Consequences

* Both machines need this deck-server; a stock Herdr cannot clone or
  transfer (the menus say so).
* Moving the main checkout alone removes nothing: it is the repository.
* The GitHub login is per machine and is `gh`'s: git run by an agent or in
  a terminal there uses it too.
* HARNESS still reads this computer's `~/.claude` itself (ADR 0002, "Not
  yet remote"); its transfer goes through this computer's deck-server.
