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

GitHub's OAuth device flow, run by the server (`deck.github.login.start` →
`{login_id, user_code, verification_uri}`, `login.status`, `status`,
`logout`). The server polls GitHub and keeps the token in
`<config dir>/github/token.json` (owner-only on Unix). A clone refused for
a github.com address is retried once over HTTPS with the token given to
that one git process in its environment (`GIT_CONFIG_*`:
`http.https://github.com/.extraheader`, credential helpers off): never on a
command line, in a remote URL or a repository's config, in a log or a
reply. Scope `repo`. The OAuth App's client id is `github::CLIENT_ID`
(empty until the app exists; `DECK_GITHUB_CLIENT_ID` overrides it).

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
* The GitHub token is per machine, used only for clones Deck starts; git
  run by an agent there does not see it.
* HARNESS still reads this computer's `~/.claude` itself (ADR 0002, "Not
  yet remote"); its transfer goes through this computer's deck-server.
