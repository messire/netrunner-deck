# ADR 0006: One installer, three kinds

Status: accepted, 2026-11-25 (implemented)

## Decision

Netrunner Deck is a client (Deck) and a server (deck-server). The Windows
installer (`src-tauri/windows/installer.nsi`, Tauri's NSIS template of
tauri-cli 2.12.4 with additions) asks which to install:

* **Client and server** (default, like Herdr): Deck, and deck-server for
  this computer's sessions.
* **Client only**: Deck, connecting to servers elsewhere (another machine,
  or Herdr).
* **Server only**: deck-server alone; Deck on other machines connects to it.

The choice is kept in `HKCU\Software\Netrunner Deck` `InstallKind` and used
again by every later install, so Deck's own updater (a passive install)
keeps it. `/KIND=client|server|both` sets it on the command line.

deck-server goes to `%LOCALAPPDATA%\deck-server\bin\deck-server.exe`, the
place its self-update replaces it, starts with the user session
(`HKCU\...\Run` `NetrunnerDeckServer` = `deck-server.exe start`) and is
started at once. It is a GUI-subsystem program on Windows, so neither the
installer nor sign-in shows a console window; typed in a terminal it
attaches to that terminal. An install stops the default session's server
for the swap (its panes come back from the snapshot); a running one of
another session is moved aside (`deck-server.old.exe`). Switching to
client only removes the server and its start entry; uninstalling removes
both (an update's uninstall step keeps them).

The release workflow bundles the Windows deck-server CI published to
`deck-server-latest` (`DECK_SERVER_EXE` at build time, checked against its
manifest). A build without it (`build.yml`, a local `tauri build`) makes a
client-only installer and shows no choice.

## Consequences

* A server-only machine has no Deck to update it: deck-server keeps itself
  up to date (ADR 0005's Windows self-update).
* The template follows Tauri's; a tauri-cli upgrade must re-apply these
  additions to its new template.
