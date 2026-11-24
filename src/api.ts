import type { UnlistenFn } from "@tauri-apps/api/event";
import { invoke, listen } from "./tauri";
import { mergeTrees, splitId, tagDeep, tagId, type Probed, type ServersView } from "./servers";

export type AgentStatus = "idle" | "working" | "blocked" | "done" | "unknown";

export interface Workspace {
  workspace_id: string;
  number: number;
  label: string;
  focused: boolean;
  pane_count: number;
  tab_count: number;
  active_tab_id: string;
  agent_status: AgentStatus;
  tokens?: Record<string, string>;
  worktree?: { repo_key: string; repo_name: string; branch?: string; is_linked_worktree?: boolean; checkout_path?: string } | null;
}
export interface Tab {
  tab_id: string;
  workspace_id: string;
  number: number;
  label: string;
  focused: boolean;
  pane_count: number;
  agent_status: AgentStatus;
}
export interface Pane {
  pane_id: string;
  terminal_id: string;
  workspace_id: string;
  tab_id: string;
  focused: boolean;
  cwd?: string | null;
  label?: string | null;
  agent?: string | null;
  display_agent?: string | null;
  title?: string | null;
  agent_status: AgentStatus;
  tokens?: Record<string, string>;
  /** The agent's own session identity (Claude Code: `{kind:"id", value:<uuid>}`), when Herdr detected one. */
  agent_session?: { source?: string; agent?: string; kind?: string; value?: string } | null;
}
export interface Agent {
  terminal_id: string;
  name?: string | null;
  agent?: string | null;
  display_agent?: string | null;
  agent_status: AgentStatus;
  tokens?: Record<string, string>;
  workspace_id: string;
  tab_id: string;
  pane_id: string;
  focused: boolean;
}
export interface Tree { workspaces: Workspace[]; tabs: Tab[]; panes: Pane[]; agents: Agent[] }
/** The merged tree of every connected server, and why the servers that failed did. */
export interface MultiTree { tree: Tree; down: Record<string, string> }

/** One Claude Code account of a server's machine (deck-server's `deck.accounts.list`, shaped like `cswap list --json`). */
export interface Account {
  number: number;
  email: string;
  alias?: string;
  organizationName?: string | null;
  active: boolean;
  usageStatus?: string;
  usage?: AccountUsage;
  lastGoodUsage?: AccountUsage;
}
export interface AccountUsage {
  fiveHour?: { pct: number; countdown?: string };
  sevenDay?: { pct: number; countdown?: string };
  scoped?: { name: string; pct: number; countdown?: string }[];
}
export interface AccountList {
  activeAccountNumber: number | null;
  accounts: Account[];
  /** False: the server keeps no accounts (a stock Herdr). */
  managed?: boolean;
  /** The machine has a live Claude login its agents run under. */
  signedIn?: boolean;
  /** After a sign-in: the number of the account it stored. */
  added?: number;
}
/** What a server without accounts (stock Herdr) shows. */
export const UNMANAGED: AccountList = { managed: false, activeAccountNumber: null, accounts: [] };

/**
 * One event from Herdr's `events.subscribe` stream, forwarded as-is by the
 * Rust pump on `herdr://event`. `event` is the variant name in snake_case
 * (`pane_updated`, `workspace_created`, …) and the remaining fields are that
 * variant's payload — e.g. `pane_updated` carries `pane: PaneInfo` and is
 * metadata, not output. See docs/architecture.md F1–F3. Routing: `src/events.ts`.
 */
export type HerdrEvent = { event: string; [k: string]: unknown };

export type TreeAction = "workspace.create" | "workspace.close" | "workspace.rename" | "tab.create" | "tab.close" | "tab.rename" | "pane.close" | "pane.split" | "pane.rename";
export type ActionOptions = { cwd?: string; label?: string; close_group?: boolean; direction?: "right" | "down" };

/**
 * `pane.read` reply. `revision` is the fork's content counter (0 upstream).
 * `cursor` and `alternate_screen` are fork additions (M2): `cursor.line` is
 * the cursor's row as an index into the lines of `text` (absent when the
 * server cannot tell), so the view places a real cursor instead of guessing.
 */
export interface PaneRead {
  text: string;
  revision: number;
  cursor?: PaneCursor | null;
  alternate_screen?: boolean;
}
export interface PaneCursor {
  line?: number | null;
  col: number;
  visible: boolean;
  /** DECSCUSR 0–6; 0 = terminal default. */
  shape: number;
}

// DEV-only call counters for manual checks: Tauri `invoke` is not visible in the
// DevTools Network tab, so inspect `window.__deckCalls` in the webview console.
declare global { interface Window { __deckCalls?: Record<string, number> } }
export const callCounts: Record<string, number> = {};
const count = (name: string) => {
  if (!import.meta.env.DEV) return;
  callCounts[name] = (callCounts[name] ?? 0) + 1;
  if (typeof window !== "undefined") window.__deckCalls = callCounts;
};

/** A newer signed release the updater found (M5). */
export type UpdateInfo = { version: string; current: string; notes: string | null; date: string | null };

/**
 * Session archive ("cold storage"): every Claude session on this computer that
 * is not open in Herdr — scanned from Herdr's session snapshots and Claude's
 * transcripts, so nothing has to be archived first. Keyed by session ids.
 */
export interface ArchivedSession { tab_label: string; agent?: string | null; session_id?: string | null; cwd: string; resumable: boolean }
export interface ArchiveEntry {
  id: string;
  source: "herdr" | "claude";
  label: string;
  repo_name?: string | null;
  branch?: string | null;
  cwd: string;
  repo_root?: string | null;
  linked_worktree: boolean;
  sessions: ArchivedSession[];
  last_active_ms: number;
  prompt?: string | null;
  prompts: number;
  summary?: string | null;
  summary_state: "pending" | "ready" | "failed" | "none";
  summary_error?: string | null;
  status: "closed" | "restored";
  restored_at_ms?: number | null;
  restored_workspace_id?: string | null;
}

/** A user-made group of repositories in the ARCHIVE tree; repos in no group are in the main group. */
export interface RepoGroup { id: string; name: string; repos: string[]; hidden: boolean }

/** One Claude Code configuration file of a repository (HARNESS view). */
export interface HarnessItem { kind: string; name: string; path: string; size: number; mtime_ms: number; local: boolean; description?: string | null; tags: string[]; argument_hint?: string | null }
export interface HarnessRepo { key: string; name: string; root: string; personal: boolean; exists: boolean; items: HarnessItem[] }

/** A harness entry pinned to the QUICKHACKS panel. */
export interface Hack { id: string; name: string; kind: string; source: "personal" | "project"; repo_key?: string | null; repo_name?: string | null; path: string; description?: string | null; argument_hint?: string | null }
export interface HacksSaved { hacks: Hack[]; collapsed: boolean }
export interface WorktreeInfo { path: string; branch?: string | null; is_bare: boolean; is_detached: boolean; is_prunable: boolean; is_linked_worktree: boolean; open_workspace_id?: string | null; label: string }
export interface QuickhackEvent { run: string; stage: "workspace" | "shell" | "agent" | "prompt" | "done" | "error"; detail: unknown }

/** UPLINK: git state of a checkout and what a pull brought in. */
export interface GitSync { branch?: string | null; upstream?: string | null; behind: number; ahead: number; dirty: boolean }
export interface UplinkCommit { sha: string; title: string; pr?: number | null; url?: string | null; author: string; at_ms: number }
export interface Update { id: string; repo: string; path: string; branch: string; from: string; to: string; at_ms: number; commits: UplinkCommit[]; seen: boolean }

/** A line-aligned slice of a Claude Code transcript (`~/.claude/projects/…/<session>.jsonl`). */
export interface TranscriptChunk { session_id: string; path: string; start: number; end: number; len: number; lines: string[] }

/** Server of a tagged id; `null` lets the Rust side pick the primary server. */
const srv = (server?: string | null) => (server ? server : null);

/**
 * A headless Claude pane reports the agent label `deck-claude` (so Deck can
 * tell it from a terminal one); everywhere it is shown it is just Claude.
 */
function claudeNamed<T>(v: T): T {
  if (Array.isArray(v)) { v.forEach(claudeNamed); return v; }
  if (v && typeof v === "object") {
    const o = v as Record<string, unknown>;
    if (o.agent === "deck-claude") o.display_agent = "Claude";
    for (const k in o) if (o[k] && typeof o[k] === "object") claudeNamed(o[k]);
  }
  return v;
}
/** Quickhack runs → the server they were launched on, to tag their progress events. */
const runServer = new Map<string, string | null>();

export const api = {
  ping: (server?: string | null) => invoke<{ version: string; protocol: number }>("herdr_ping", { server: srv(server) }),
  /**
   * Every connected server's tree, ids tagged `<server>|<id>` (see `servers.ts`).
   * Rejects only when every server failed; the others' errors are in `down`.
   */
  tree: async (servers: string[]): Promise<MultiTree> => {
    count("herdr_tree");
    const results = await Promise.allSettled(servers.map((server) => invoke<Tree>("herdr_tree", { server })));
    const parts: { server: string; tree: Tree }[] = [];
    const down: Record<string, string> = {};
    results.forEach((r, i) => { if (r.status === "fulfilled") parts.push({ server: servers[i], tree: claudeNamed(r.value) }); else down[servers[i]] = String(r.reason); });
    if (servers.length && !parts.length) throw new Error(Object.values(down)[0]);
    return { tree: mergeTrees(parts), down };
  },
  paneRead: (paneKey: string, lines = 400) => {
    count("herdr_pane_read");
    const { server, id } = splitId(paneKey);
    return invoke<{ read: PaneRead }>("herdr_pane_read", { server: srv(server), paneId: id, lines });
  },
  sendText: (paneKey: string, text: string) => { const { server, id } = splitId(paneKey); return invoke("herdr_pane_send_text", { server: srv(server), paneId: id, text }); },
  sendKeys: (paneKey: string, keys: string[]) => { const { server, id } = splitId(paneKey); return invoke("herdr_pane_send_keys", { server: srv(server), paneId: id, keys }); },
  /**
   * deck-server's headless Claude sessions (docs/adr/0003-headless-claude.md):
   * `send {text, images?}`, `respond {request_id, response}`, `interrupt`,
   * `set {model?, permission_mode?}` on the pane's server.
   */
  agentCall: (paneKey: string, method: "send" | "respond" | "interrupt" | "set" | "resume" | "restart", params: Record<string, unknown> = {}) => {
    const { server, id } = splitId(paneKey);
    return invoke<unknown>("agent_call", { server: srv(server), method, params: { ...params, pane_id: id } });
  },
  /** Stream a headless pane's events (`onAgent`) until `agentUnwatch`. */
  agentWatch: (paneKey: string) => { const { server, id } = splitId(paneKey); return invoke<void>("agent_watch", { server: srv(server), paneId: id }); },
  agentUnwatch: (paneKey: string) => { const { server, id } = splitId(paneKey); return invoke<void>("agent_unwatch", { server: srv(server), paneId: id }); },
  /** One line of a headless pane's stream, with the pane's (tagged) id. */
  onAgent: (cb: (paneKey: string, line: unknown) => void): Promise<UnlistenFn> =>
    listen<{ server?: string | null; pane_id: string; line: unknown }>("deck://agent", (e) =>
      cb(e.payload.server ? tagId(e.payload.server, e.payload.pane_id) : e.payload.pane_id, e.payload.line)),
  focus: (target: "workspace" | "tab" | "pane", key: string) => { const { server, id } = splitId(key); return invoke("herdr_focus", { server: srv(server), target, id }); },
  /**
   * Tree actions (M3); the Rust side maps the name to a Herdr method with a fixed param set.
   * The server is the id's; an action without an id (a new space) names it in `server`.
   * Returned ids are tagged.
   */
  action: async (action: TreeAction, key?: string | null, options?: ActionOptions, server?: string | null) => {
    const parts = key ? splitId(key) : { server: server ?? "", id: null };
    const s = parts.server || server || null;
    const r = await invoke<unknown>("herdr_action", { server: srv(s), action, id: parts.id ?? null, options: options ?? null });
    return s ? tagDeep(s, r) : r;
  },
  /** SERVERS (preferences): what is connected, what discovery found. */
  servers: () => invoke<ServersView>("servers_list"),
  serversScan: () => invoke<ServersView>("servers_scan"),
  serversSeen: () => invoke<ServersView>("servers_seen"),
  serverConnect: (id: string) => invoke<ServersView>("server_connect", { id }),
  serverDisconnect: (id: string) => invoke<ServersView>("server_disconnect", { id }),
  serverRename: (id: string, name: string) => invoke<ServersView>("server_rename", { id, name }),
  serverAdd: (address: string) => invoke<ServersView>("server_add", { address }),
  serverForget: (id: string) => invoke<ServersView>("server_forget", { id }),
  /** CHECK in the ADD dialog: the servers on that machine that answer through its bridge. */
  serverProbe: (user: string, host: string, port: number) => invoke<Probed[]>("server_probe", { user, host, port }),
  /** Save the machine and connect the servers at `sockets` on it. */
  serverAddHost: (user: string, host: string, port: number, sockets: string[]) => invoke<ServersView>("server_add_host", { user, host, port, sockets }),
  /** Drop a machine (`user@host[:port]`) and all its servers; its bridge closes. */
  serverRemoveHost: (machine: string) => invoke<ServersView>("server_remove_host", { machine }),
  onServers: (cb: () => void): Promise<UnlistenFn> => listen("deck://servers", () => cb()),
  /** Clipboard text for ctrl+v / shift+insert; empty when the clipboard holds no text. */
  clipboardReadText: () => invoke<string>("clipboard_read_text"),
  /** The pane selection for ctrl+c / ctrl+insert / the pane menu's COPY. */
  clipboardWriteText: (text: string) => invoke<void>("clipboard_write_text", { text }),
  /** One line into `<app log dir>/deck-diag.log` (pane view consistency check). */
  diagLog: (line: string) => invoke<void>("diag_log", { line }),
  /** OS notification (M4); used when an agent starts waiting while its pane is not on screen. */
  notify: (title: string, body: string) => invoke<void>("notify", { title, body }),
  /** Updater (M5): `null` when up to date; errors (offline, no release yet) are advisory. */
  updateCheck: () => invoke<UpdateInfo | null>("update_check"),
  updateInstall: () => invoke<void>("update_install"),
  archiveList: () => invoke<ArchiveEntry[]>("archive_list"),
  archiveClose: (workspaceKey: string, closeGroup = false) => { const { server, id } = splitId(workspaceKey); return invoke<{ id: string | null }>("archive_close", { server: srv(server), workspaceId: id, closeGroup }); },
  /** Reopen on `server` (`null` = the primary one); the returned workspace id is tagged. */
  archiveRestore: async (id: string, server?: string | null) => {
    const r = await invoke<{ workspace_id: string; resumed: number }>("archive_restore", { id, server: srv(server) });
    return server ? tagDeep(server, r) : r;
  },
  archiveDelete: (id: string, removeWorktree = false) => invoke<{ removed_worktree: string | null }>("archive_delete", { id, removeWorktree }),
  archiveSummarize: (id: string) => invoke<void>("archive_summarize", { id }),
  archiveGroupsGet: () => invoke<RepoGroup[]>("archive_groups_get"),
  archiveGroupsSet: (groups: RepoGroup[]) => invoke<RepoGroup[]>("archive_groups_set", { groups }),
  onArchive: (cb: () => void): Promise<UnlistenFn> => listen("deck://archive", () => cb()),
  /** http(s) links from chat/harness markdown open in the system browser, never in the webview. */
  openExternal: (url: string) => invoke<void>("open_external", { url }),
  uplinkSync: (paths: string[]) => invoke<Record<string, GitSync>>("uplink_sync", { paths }),
  uplinkFetch: (roots: string[]) => invoke<Record<string, string>>("uplink_fetch", { roots }),
  uplinkPull: (repo: string, path: string) => invoke<Update>("uplink_pull", { repo, path }),
  uplinkList: () => invoke<{ updates: Update[] }>("uplink_list"),
  uplinkSeen: () => invoke<{ updates: Update[] }>("uplink_seen"),
  uplinkRemove: (id: string) => invoke<{ updates: Update[] }>("uplink_remove", { id }),
  onUplink: (cb: () => void): Promise<UnlistenFn> => listen("deck://uplink", () => cb()),
  /** Sidebar "+": open a repository folder as a workspace; a new linked worktree in a repository. */
  repoOpen: async (path: string, label: string | undefined, server: string) =>
    tagDeep(server, await invoke<{ workspace_id: string; pane_id: string }>("repo_open", { server, path, label: label ?? null })),
  worktreeCreate: async (cwd: string, branch: string, base: string | undefined, server: string) =>
    tagDeep(server, await invoke<{ workspace_id: string; pane_id: string; path?: string | null }>("worktree_create", { server, cwd, branch, base: base ?? null })),
  quickhacksGet: () => invoke<HacksSaved>("quickhacks_get"),
  quickhacksSet: (saved: HacksSaved) => invoke<HacksSaved>("quickhacks_set", { saved }),
  /** `worktree.list` of the repository containing `cwd`. */
  quickhackWorktrees: async (cwd: string, server?: string | null) => {
    const r = await invoke<{ worktrees: WorktreeInfo[]; source?: { repo_root?: string } }>("quickhack_worktrees", { server: srv(server), cwd });
    return server ? tagDeep(server, r) : r;
  },
  /** Open a session in `cwd`, start `agent`, send `prompt`; progress on `deck://quickhack` keyed by `run`. */
  quickhackLaunch: async (run: string, launch: { cwd: string; repo_root: string; label?: string; agent: string; prompt: string }, server?: string | null) => {
    runServer.set(run, srv(server));
    const r = await invoke<{ workspace_id: string; pane_id: string }>("quickhack_launch", { server: srv(server), run, launch });
    return server ? tagDeep(server, r) : r;
  },
  onQuickhack: (cb: (e: QuickhackEvent) => void): Promise<UnlistenFn> =>
    listen<QuickhackEvent>("deck://quickhack", (e) => { const s = runServer.get(e.payload.run); cb(s ? tagDeep(s, e.payload) : e.payload); }),
  /** Personal (`~/.claude`) first, then the given repositories' Claude Code files. */
  harnessScan: (repos: { key: string; name: string; root: string }[]) => invoke<HarnessRepo[]>("harness_scan", { repos }),
  harnessRead: (path: string) => invoke<string>("harness_read", { path }),
  /** Tag the untagged items of one repo (shadow claude -p → `tags:` in each file); false when already running. */
  harnessClassify: (key: string, items: HarnessItem[]) => invoke<boolean>("harness_classify", { key, items }),
  onHarness: (cb: (e: { key: string; written?: number; error?: string }) => void): Promise<UnlistenFn> =>
    listen<{ key: string; written?: number; error?: string }>("deck://harness", (e) => cb(e.payload)),
  /** `from`: continue after this byte offset; `tail`: only the last N bytes (initial load). */
  transcriptRead: (sessionId: string, from?: number, tail?: number, server?: string | null) =>
    invoke<TranscriptChunk>("transcript_read", { server: srv(server), sessionId, from: from ?? null, tail: tail ?? null }),
  /** Transcript modified time (ms) per session id; sessions without a transcript are absent. */
  transcriptMtimes: (sessionIds: string[], server?: string | null) => invoke<Record<string, number>>("transcript_mtimes", { server: srv(server), sessionIds }),
  /** Tell the poller which transcript to watch for growth (`null` = none). */
  transcriptWatch: (sessionId: string | null, server?: string | null) => invoke<void>("transcript_watch", { server: srv(server), sessionId }),
  onTranscript: (cb: (e: { session_id: string; len: number }) => void): Promise<UnlistenFn> =>
    listen<{ session_id: string; len: number }>("deck://transcript", (e) => cb(e.payload)),
  /** What is at `path` on `server`'s machine (`~` expanded there). */
  folderState: (server: string | null, path: string) => invoke<{ path: string; exists: boolean; dir: boolean }>("fs_call", { server: srv(server), method: "stat", path }),
  /** Create the folder (with parents) on `server`'s machine. */
  folderCreate: (server: string | null, path: string) => invoke<{ path: string; exists: boolean; dir: boolean }>("fs_call", { server: srv(server), method: "mkdir", path }),
  /** Folders Claude Code knows on `server`'s machine (deck-server's `deck.repos.list`). */
  reposList: (server: string | null) => invoke<{ folders: { name: string; root: string; git: boolean }[] }>("repos_list", { server: srv(server) }),
  /** The accounts of `server`'s machine (deck-server keeps them; Deck only asks). */
  accountsList: (server: string | null) => { count("accounts_call"); return invoke<AccountList>("accounts_call", { server: srv(server), method: "list", params: {} }); },
  accountsSwitch: (server: string | null, number: number) => invoke<AccountList>("accounts_call", { server: srv(server), method: "switch", params: { number } }),
  accountsRemove: (server: string | null, number: number) => invoke<AccountList>("accounts_call", { server: srv(server), method: "remove", params: { number } }),
  /** Starts Claude's sign-in on the server: the link to open and the id `accountsLoginFinish` needs. */
  accountsLoginStart: (server: string | null) => invoke<{ login_id: string; url: string }>("accounts_call", { server: srv(server), method: "login.start", params: {} }),
  accountsLoginFinish: (server: string | null, loginId: string, code: string, activate = false) =>
    invoke<AccountList>("accounts_call", { server: srv(server), method: "login.finish", params: { login_id: loginId, code, activate } }),
  // The payload is whatever Herdr sent plus `server`; its ids are tagged here
  // and `routeEvent` validates the shape.
  onEvent: (cb: (event: HerdrEvent, server: string | null) => void): Promise<UnlistenFn> =>
    listen<HerdrEvent>("herdr://event", (e) => {
      const server = typeof e.payload?.server === "string" ? e.payload.server : null;
      claudeNamed(e.payload);
      cb(server ? tagDeep(server, e.payload) : e.payload, server);
    }),
  onOffline: (cb: (server: string | null, message: string) => void): Promise<UnlistenFn> =>
    listen<{ server?: string; error?: string } | string>("herdr://offline", (e) =>
      typeof e.payload === "string" ? cb(null, e.payload) : cb(e.payload.server ?? null, e.payload.error ?? "offline")),
};

export function accountLabel(a: Account): string {
  const alias = (a.alias ?? "").trim();
  if (alias) return alias.toUpperCase();
  const [user, domain = ""] = a.email.split("@");
  const org = domain.split(".")[0];
  return (/gmail|proton|outlook|hotmail|yahoo/i.test(org) ? user : org).toUpperCase();
}

export function usageOf(a: Account) {
  const u = a.usage ?? a.lastGoodUsage ?? {};
  const pct = (x?: number) => (typeof x === "number" && isFinite(x) ? Math.max(0, Math.min(100, Math.round(x))) : null);
  return {
    five: pct(u.fiveHour?.pct), fiveReset: u.fiveHour?.countdown,
    week: pct(u.sevenDay?.pct), weekReset: u.sevenDay?.countdown,
    scoped: (u.scoped ?? []).map((s) => ({ name: s.name, pct: pct(s.pct), reset: s.countdown })),
  };
}

export type Zone = "safe" | "public" | "restricted" | "danger";
export function zoneOf(a?: Account): Zone {
  if (!a) return "danger";
  if (a.usageStatus && /relogin|expired|error/i.test(a.usageStatus)) return "danger";
  const { five, week } = usageOf(a);
  const worst = Math.max(five ?? 0, week ?? 0);
  if (worst >= 100) return "danger";
  if (worst >= 80) return "restricted";
  if (worst >= 50) return "public";
  return "safe";
}
