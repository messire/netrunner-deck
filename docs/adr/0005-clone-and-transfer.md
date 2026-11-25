# ADR 0005: Clone, GitHub sign-in and transfers are the server's work

Status: accepted, 2026-11-25 (implemented: server and Deck)

Builds on [ADR 0002](0002-one-connection-type.md) (one connection type) and
[ADR 0004](0004-chat-only-panes-and-accounts.md) (each machine keeps its own
accounts).

## Decision

Deck never touches a disk or runs git, not even on its own machine: every
clone, sign-in and transfer is a `deck.*` method of the server of the
machine concerned, reached over its one connection like `deck.fs.*`. The
work lives in `server/crates/deck-accounts` (`clone`, `github`, `transfer`,
`pack`, `jobs`, `proc`) and is answered by `server/src/api/machine.rs`
before Herdr's typed parse. Deck calls it through one Tauri command
(`machine_call`, an allow-list in `src-tauri/src/machine.rs`).

Anything long is a **job**: `*.start` returns `{job}` at once, the work runs
on its own thread, `*.status {job}` returns `{state, phase, percent,
message, …}`. A job outlives the dialog that started it; Deck's polling
loops (`src/jobs.ts`) are not tied to a component either, so what was
started still finishes (a clone still opens, a transfer still opens on the
target).

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
