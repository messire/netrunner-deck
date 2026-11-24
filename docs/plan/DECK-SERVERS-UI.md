# Deck servers and preferences: UI spec

Status: implemented on `feat/deck-servers-ui` (see "Implementation notes"). Mockups: https://claude.ai/artifact/WwhMhdz2w3mke8Qd5DKoAJ

Deck connects to several compatible servers at once (today local `herdr` and
`deck-server`, later servers on other machines) and shows their sessions in one
tree.

## Decisions

1. **No auto-connect.** Deck discovers local servers and *offers* them; it
   connects only when the user clicks CONNECT. Connected servers are remembered
   and reconnect on the next start.
2. **Settings live in `preferences.json`** in Deck's data dir, next to
   `archive.json` (moves with `DECK_DATA_DIR`).
3. **The server menu opens by clicking the active NETRUNNER DECK tab.**

## Core model

- **Connection** is set in preferences. Deck keeps every connected server live
  at once: events, agent status and "blocked" notifications come from all.
- **Visibility** is a filter on the NETRUNNER DECK tab: ALL or one server. It
  does not reconnect anything, so switching is instant.

## 1. Preferences button

- A ⚙ cell left of the clock, same row as the account zone; also `Ctrl+,`.
- Orange diamond on it: a connected server is unreachable.
- Cyan dot on it: a newly found server can be connected.

## 2. Preferences: SERVERS

- Modal panel in the palette style (Esc closes, focus returns). Left nav with
  sections; only SERVERS for now, others shown as "soon".
- Header: counts (connected, online, found) and FIND AGAIN.
- Two groups:
  - **Connected**: switch on, name (renamable), pipe address, kind
    (`herdr` / `deck-server`), version and protocol, pane count.
  - **Found, not connected**: switch off, "new" badge, CONNECT button.
- Errors sit in the server's own row with a fix hint (access denied → server
  runs elevated, use `allow_unelevated_clients`; old protocol → what to
  update, switch disabled).
- **+ ADD** opens a dialog: user, host or address, port (22 by default, 1-65535).
  CHECK lists that machine's servers over SSH and pings each one through the
  machine's bridge (the same path every connection takes, ADR 0002); the dialog
  shows only the servers that answered, with kind and version, each with a
  checkbox (deck-server's root session pre-checked, else Herdr's). ADD & CONNECT
  saves the machine and connects the checked ones. SSH timeout, refused key,
  unknown host or a machine without deck-server are shown in the dialog in plain
  words, the raw error under them.
- **Remove**: every row on another machine has ✕; a second click (REMOVE
  MACHINE?) drops all of that machine's servers from `preferences.json`,
  disconnects them, closes its bridge, and the machine is no longer scanned. A
  manually added local address has ✕ too. Local found servers need none: they
  drop out when they stop answering.
- **Remote scan**: each saved machine is listed and every socket pinged through
  its bridge; only servers that answer are offered. A machine that cannot be
  reached does not keep its old rows: it shows as one grey row under MACHINES
  NOT REACHABLE with the reason, RETRY and ✕. Connected servers on it keep
  their own OFFLINE state.
- Discovery: in the app dirs `herdr`, `herdr-dev`, `deck-server`,
  `deck-server-dev`, ping the root `herdr.sock` and `sessions\*\herdr.sock`.
  Runs when preferences open, on FIND AGAIN and once a minute in the
  background. Spawns nothing.
- Default name: kind + session (`herdr`, `herdr · decktest`,
  `deck-server · try`). Server id = socket path, so renames are safe.
- Turning a server off moves it back to "found" and it no longer counts as new.
  A vanished found server drops out; a vanished connected one stays marked
  "not found".

## 3. Which server is shown

- The active tab shows the scope: `▞ NETRUNNER DECK ALL 2/2 ▾` or the server
  name. Clicking an inactive tab switches views as today; clicking the active
  one opens the menu.
- Menu: ALL SERVERS, each connected server (status, version, pane count,
  `Alt+1…9`), a "Found, not connected" group with `+ CONNECT`, and
  "Manage servers…" (`Ctrl+,`). `Alt+0` = ALL.
- Tree: in ALL it splits into one section per server; `+` inside a section
  creates on that server, `+` at the top asks which server (not with one
  server). With one server there are no sections and it looks like today.
- The pane header gets a server prefix in ALL: `herdr ▸ workspace ▸ tab`.

## States

| Situation | Tab and menu | Tree | Preferences |
|---|---|---|---|
| One server | No scope label, only ▾ | As today | One row |
| First run, servers found | `NO SERVER`, menu shows found group | Empty state "Connect a server": found servers with CONNECT, then Preferences | All rows under Found |
| No servers at all | `NO SERVER` in red | "No servers found. Start Herdr or deck-server" + Preferences | Empty list, add field focused |
| New server found | Cyan dot on ⚙, entry in found group | Unchanged | Row with "new" and CONNECT |
| One of two dropped | `ALL 1/2`, orange diamond | Section red, OFFLINE, last tree dimmed and inert | Error + RETRY in its row |
| Selected server dropped | Label red: `herdr · OFFLINE` | Dimmed tree, "no link to herdr" bar; selection kept | As above |
| Access denied (elevated) | As dropped | As dropped | `allow_unelevated_clients` hint |
| Incompatible protocol | Item grey, disabled | Not shown | Switch disabled, what to update |

The bottom "HERDR OFFLINE" bar remains only for "no connected server online".

## Keyboard and a11y

- `Ctrl+,` opens preferences, Esc closes and restores focus.
- Focused tab: Enter / Space / ↓ open the menu; ↑↓ Enter inside, Esc closes.
  Roles `menu` / `menuitemradio`, `aria-expanded` on the tab.
- Status is carried by shape and text, not color alone (filled/empty diamond,
  OFFLINE, 1/2).

## Code implications

- One `Runtime` instance per server (S0 already allows it) plus a server
  registry: discovery, saved preferences, per-server status.
- Pane ids collide across servers (`w1:p1` exists on both), so the frontend key
  becomes **server + id**. This is the `machine` dimension from S6, earlier.
- An event stream per server; events are tagged with their server, reconnects
  are independent.
- Archive, quickhacks and restore gain a `server` field; restore opens on the
  original server or asks if it is gone.

## Implementation notes

- Rust: `src-tauri/src/servers.rs` (registry, discovery, `preferences.json`,
  commands `servers_*` / `server_*`). Server ids are `herdr`,
  `herdr-dev/decktest`, `deck-server/try`; an address outside the known app
  dirs is its own id. Every `herdr_*` command takes an optional `server`
  (none = the primary server: the first connected one that is online).
- Events: `herdr://event` carries `server`; `herdr://offline` is
  `{server, error}`; `deck://servers` fires on any registry change.
- Webview: `src/servers.ts` tags ids as `<server>|<id>` at the API boundary
  (`api.ts`), so the rest of the UI works on plain strings.
- No `preferences.json` yet (an existing single-server Deck) = the default
  Herdr (or `HERDR_SOCKET_PATH`) is connected, as before. Everything found
  later is only offered.
- A webview newer than its Rust side (`tauri dev` with an old binary) falls
  back to one unnamed server: the single-server Deck.
- Machines: `server_probe(user, host, port)` (CHECK), `server_add_host(user,
  host, port, sockets)` (ADD & CONNECT), `server_remove_host(machine)` (REMOVE
  MACHINE); `servers_list` carries `unreachable` (machine + reason) and each
  remote row its `machine`. The old `server_add(address)` still takes a typed
  or stored address.
- Not done yet: the protocol check ("incompatible" state) — no minimum
  protocol is defined; the archive still reads the primary server only and
  restores onto the shown server (or the primary).
