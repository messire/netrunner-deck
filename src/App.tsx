import { createEffect, createMemo, createResource, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { getCurrentWindow, inTauri } from "./tauri";
import { api, type ArchiveEntry, type GitSync, type Hack, type RepoGroup, type UpdateInfo, type Tree, type CswapList, type Tab, type Pane, type Workspace, type CswapAccount, type TreeAction, type ActionOptions, zoneOf, usageOf, accountLabel } from "./api";
import Sidebar, { type SidebarSection, type SwitchError } from "./components/Sidebar";
import ServerMenu, { dotOf, type Scope } from "./components/ServerMenu";
import Prefs from "./components/Prefs";
import { connectedServers, filterTree, keepDown, LEGACY_SERVERS, migrateKey, offeredServers, serverOf, type ServersView } from "./servers";
import TabBar, { Clock, Pulse } from "./components/TabBar";
import TerminalView from "./components/TerminalView";
import ChatView from "./components/ChatView";
import KeyBar from "./components/KeyBar";
import ContextMenu, { type MenuState } from "./components/ContextMenu";
import Palette, { type PaletteItem } from "./components/Palette";
import ArchiveView from "./components/ArchiveView";
import PromptBox from "./components/PromptBox";
import NewTabBox, { type TabKind } from "./components/NewTabBox";
import QuickHacks from "./components/QuickHacks";
import NewRepoBox, { type KnownRepo } from "./components/NewRepoBox";
import NewWorktreeBox from "./components/NewWorktreeBox";
import HarnessView from "./components/HarnessView";
import UplinkView from "./components/UplinkView";
import { harnessRepos } from "./harness";
import { createLatestGate } from "./latest";
import { debounceTrailing, outputPaneOf, routeEvent } from "./events";
import { agentName, commands, hotkeyOf, nextBlockedPane, type Command } from "./actions";

/** UI-only preferences in the webview's storage; never session state (that lives in Herdr). */
function remembered(key: string): string | null {
  try { return localStorage.getItem(key); } catch { return null; }
}
function remember(key: string, value: string | null) {
  try { if (value === null) localStorage.removeItem(key); else localStorage.setItem(key, value); } catch { /* storage unavailable */ }
}

function loadActivity(): Map<string, number> {
  try { return new Map(Object.entries(JSON.parse(remembered("deck.activity") ?? "{}") as Record<string, number>)); } catch { return new Map(); }
}
function saveActivity(m: Map<string, number>) {
  // Keep it bounded: the 200 most recent panes.
  const top = [...m.entries()].sort((a, b) => b[1] - a[1]).slice(0, 200);
  remember("deck.activity", JSON.stringify(Object.fromEntries(top)));
}

const SIDE_MIN = 320;
/** Panel widths (px): both side panels share the minimum and may grow to a third of the window — three open panels split it evenly. */
function clampSide(px: number) { return Math.max(SIDE_MIN, Math.min(px, Math.floor(window.innerWidth / 3))); }

export default function App() {
  const [sideWidth, setSideWidth] = createSignal(clampSide(Number(remembered("deck.sideWidth")) || SIDE_MIN));
  const startSideDrag = (e: PointerEvent) => {
    e.preventDefault();
    const move = (ev: PointerEvent) => setSideWidth(clampSide(ev.clientX));
    const up = () => { window.removeEventListener("pointermove", move); window.removeEventListener("pointerup", up); remember("deck.sideWidth", String(sideWidth())); };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };
  // The QUICKHACKS panel resizes from its left edge under the same limits.
  const [hacksWidthRaw, setHacksWidthRaw] = createSignal(clampSide(Number(remembered("deck.hacksWidth")) || SIDE_MIN));
  const startHacksDrag = (e: PointerEvent) => {
    e.preventDefault();
    const move = (ev: PointerEvent) => setHacksWidthRaw(clampSide(window.innerWidth - ev.clientX));
    const up = () => { window.removeEventListener("pointermove", move); window.removeEventListener("pointerup", up); remember("deck.hacksWidth", String(hacksWidthRaw())); };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };
  onMount(() => { const onResize = () => { setSideWidth((w) => clampSide(w)); setHacksWidthRaw((w) => clampSide(w)); }; window.addEventListener("resize", onResize); onCleanup(() => window.removeEventListener("resize", onResize)); });
  const [selectedPane, setSelectedPaneRaw] = createSignal<string | null>(remembered("deck.selectedPane"));
  const setSelectedPane = (id: string | null) => { setSelectedPaneRaw(id); remember("deck.selectedPane", id); };
  // Panes the user switched to the raw terminal (chat is the default for a
  // pane with a Claude session). Per pane, forgotten with the pane.
  const [termPanes, setTermPanes] = createSignal<Set<string>>(new Set());
  const setPaneView = (id: string, view: "chat" | "term") =>
    setTermPanes((s) => { const n = new Set(s); if (view === "term") n.add(id); else n.delete(id); return n; });
  /** The Claude session id behind a pane, when Herdr identified one (`agent_session`). */
  const claudeSession = (id: string) => {
    const x = tree()?.panes.find((p) => p.pane_id === id);
    const s = x?.agent_session;
    return x && (x.agent ?? "").toLowerCase().includes("claude") && s?.kind === "id" && s.value ? s.value : null;
  };
  const [offline, setOffline] = createSignal<string | null>(null);

  // ---- servers (docs/plan/DECK-SERVERS-UI.md) --------------------------------
  // Connection lives in PREFERENCES; the deck tab's menu only filters what is shown.
  const [servers, { refetch: refetchServers, mutate: setServers }] = createResource<ServersView | undefined>(async (_, { value }) => {
    try { return await api.servers(); } catch (e) {
      // A backend without SERVERS (an older Rust side under `tauri dev` with a
      // newer webview): one unnamed server, i.e. the single-server Deck.
      return value ?? (/servers_list/.test(String(e)) ? LEGACY_SERVERS : undefined);
    }
  });
  const connected = () => connectedServers(servers());
  const offered = () => offeredServers(servers());
  /** Why each connected server's last tree fetch or event stream failed. */
  const [down, setDown] = createSignal<Record<string, string>>({});
  const [scope, setScopeRaw] = createSignal<Scope>(remembered("deck.scope") ?? "all");
  const setScope = (s: Scope) => { setScopeRaw(s); remember("deck.scope", s === "all" ? null : s); };
  /** The chosen scope while that server is connected, else ALL (the choice itself is kept). */
  const shown = (): Scope => { const s = scope(); return s !== "all" && connected().some((x) => x.id === s) ? s : "all"; };
  const isDown = (id: string) => !!down()[id] || connected().find((s) => s.id === id)?.status.state === "offline";
  /** Where a new space goes when nothing else says: the shown server, else the first online one. */
  const targetServer = () => { const s = shown(); if (s !== "all") return s; return (connected().find((x) => !isDown(x.id)) ?? connected()[0])?.id ?? null; };
  const [srvMenu, setSrvMenu] = createSignal(false);
  const [prefsOpen, setPrefsOpen] = createSignal(false);
  const [scanning, setScanning] = createSignal(false);
  const [prefsError, setPrefsError] = createSignal<string | null>(null);
  const serverCall = (work: Promise<ServersView>) =>
    work.then((v) => { setServers(v); setPrefsError(null); }).catch((e) => setPrefsError(String(e)));
  const scanServers = () => { setScanning(true); serverCall(api.serversScan()).finally(() => setScanning(false)); };
  const openPrefs = () => { setSrvMenu(false); setMenu(null); setOverlay(null); setPrefsOpen(true); scanServers(); };
  // Found servers stop being "new" (the gear's cyan dot) once the list was seen.
  const closePrefs = () => { setPrefsOpen(false); serverCall(api.serversSeen()); };
  const connectServer = (id: string) => serverCall(api.serverConnect(id));
  const [accountsError, setAccountsError] = createSignal<string | null>(null);
  // Account number of the `cswap switch` in flight (null = idle) and the last switch failure.
  const [switching, setSwitching] = createSignal<number | null>(null);
  const [switchError, setSwitchError] = createSignal<SwitchError | null>(null);

  // Fetchers never reject: a Solid resource in the errored state throws on read,
  // which would take the whole render down. Errors go to signals; the previous
  // value is kept so the UI stays populated while Herdr/cswap are unreachable.
  // Only the latest fetch of each resource may touch the status signals: a
  // slow, stale `herdr_tree` finishing after a fresh one must not flip OFFLINE.
  const gate = createLatestGate();
  // Every connected server's tree, merged (ids `<server>|<id>`). A server that
  // fails keeps its last known part, shown dimmed in its section.
  // JSON, not a join: the single-server fallback's one id is "".
  const connectedKey = () => (servers() ? JSON.stringify(connected().map((s) => s.id)) : false);
  const [fullTree, { refetch: refetchTree }] = createResource<Tree | undefined, string | false>(connectedKey, async (key, { value }) => {
    const seq = gate.begin("tree");
    const ids = JSON.parse(String(key)) as string[];
    try {
      const r = await api.tree(ids);
      if (!gate.isCurrent("tree", seq)) return value;
      setDown(r.down);
      setOffline(null);
      return keepDown(r.tree, value, Object.keys(r.down));
    } catch (e) {
      if (!gate.isCurrent("tree", seq)) return value;
      setDown(Object.fromEntries(ids.map((id) => [id, String(e)])));
      setOffline(String(e));
      return value;
    }
  });
  /** What the deck shows: ALL servers or the one picked on the tab. */
  const tree = createMemo(() => filterTree(fullTree(), shown() === "all" ? null : shown()));
  const panesOf = (id: string) => fullTree()?.panes.filter((x) => serverOf(x.pane_id) === id).length ?? 0;
  const sections = createMemo((): SidebarSection[] =>
    shown() === "all" && connected().length > 1
      ? connected().map((s) => ({
          id: s.id, name: s.name,
          error: down()[s.id] ?? (s.status.state === "offline" ? s.status.error ?? "offline" : null),
          connecting: s.status.state === "connecting",
          tree: filterTree(fullTree(), s.id),
        }))
      : []);
  // A pane remembered by a single-server Deck (no server in its id) is the primary server's.
  createEffect(() => {
    const first = connected()[0]?.id;
    const sel = selectedPane();
    if (first && sel && !sel.includes("|")) setSelectedPane(migrateKey(sel, first));
  });
  const [accounts, { refetch: refetchAccounts }] = createResource<CswapList | undefined>(async (_, { value }) => {
    const seq = gate.begin("accounts");
    try {
      const l = await api.cswapList();
      if (!gate.isCurrent("accounts", seq)) return value;
      setAccountsError(null);
      setSwitchError(null);
      return l;
    } catch (e) {
      if (!gate.isCurrent("accounts", seq)) return value;
      setAccountsError(String(e));
      return value;
    }
  });

  const switchAccount = (n: number) => {
    if (switching() !== null) return; // one `cswap switch` at a time
    setSwitching(n);
    setSwitchError(null);
    api.cswapSwitch(n)
      .catch((e) => setSwitchError({ number: n, message: String(e) }))
      .finally(() => { setSwitching(null); refetchAccounts(); });
  };

  // Pick the focused pane of the focused workspace when nothing is selected yet.
  const currentPane = () => {
    const t = tree();
    if (!t) return null;
    const explicit = selectedPane();
    if (explicit && t.panes.some((p) => p.pane_id === explicit)) return explicit;
    const ws = t.workspaces.find((w) => w.focused) ?? t.workspaces[0];
    const pane = t.panes.find((p) => p.workspace_id === ws?.workspace_id && p.focused) ?? t.panes[0];
    return pane?.pane_id ?? null;
  };
  const selectPane = (id: string) => { setSelectedPane(id); api.focus("pane", id).catch(() => {}); };

  // ---- M3: tree actions, context menus, palette, hotkeys -------------------
  // Action failures (Herdr refused, link down) show in the title bar for a while.
  const [actionError, setActionError] = createSignal<string | null>(null);
  let errorTimer: number | undefined;
  const showError = (m: string) => {
    setActionError(m);
    clearTimeout(errorTimer);
    errorTimer = window.setTimeout(() => setActionError(null), 8000);
  };
  // M5: a newer signed release is offered in the title bar; nothing installs by itself.
  // `tauri dev` skips the check (it would offer the GitHub build over the dev binary).
  const [update, setUpdate] = createSignal<UpdateInfo | null>(null);
  const [installing, setInstalling] = createSignal(false);
  const checkUpdate = () => {
    if (inTauri && import.meta.env.DEV) return;
    api.updateCheck().then(setUpdate).catch((e) => console.info("[update] check skipped:", String(e)));
  };
  const installUpdate = () => {
    if (installing()) return;
    setInstalling(true);
    api.updateInstall().catch((e) => { setInstalling(false); showError(`update: ${e}`); });
  };
  const runAction = (action: TreeAction, id?: string | null, options?: ActionOptions) =>
    api.action(action, id, options)
      .then(() => refetchTree())
      .catch((e) => {
        const m = String(e);
        // A primary worktree workspace needs explicit group intent; ask once more with it.
        if (action === "workspace.close" && m.includes("workspace_group_close_required") && !options?.close_group) {
          showError("space has linked worktrees — right-click again and confirm to close the group");
          return;
        }
        showError(`${action}: ${m}`);
      });

  // ---- ARCHIVE (cold storage): closed spaces that can be restored ----------
  type View = "deck" | "harness" | "archive" | "uplink";
  const [view, setViewRaw] = createSignal<View>((["harness", "archive", "uplink"] as const).find((v) => v === remembered("deck.view")) ?? "deck");
  const setView = (v: View) => { setViewRaw(v); remember("deck.view", v); };
  const [archive, { refetch: refetchArchive }] = createResource<ArchiveEntry[] | undefined>(async (_, { value }) => {
    try { return await api.archiveList(); } catch (e) { showError(`archive: ${e}`); return value; }
  });
  const [archiveBusy, setArchiveBusy] = createSignal<string | null>(null);
  const [archiveError, setArchiveError] = createSignal<string | null>(null);
  const archiveRun = (id: string, work: Promise<unknown>, after?: () => void) => {
    setArchiveBusy(id);
    setArchiveError(null);
    work.then(() => { after?.(); }).catch((e) => setArchiveError(String(e))).finally(() => { setArchiveBusy(null); refetchArchive(); });
  };
  const archiveClose = (w: Workspace, closeGroup = false) =>
    api.archiveClose(w.workspace_id, closeGroup)
      .then(() => { refetchTree(); refetchArchive(); })
      .catch((e) => {
        const m = String(e);
        if (m.includes("workspace_group_close_required") && !closeGroup) {
          showError("space has linked worktrees — use CLOSE · ARCHIVE + WORKTREES to close the group");
          return;
        }
        showError(`archive: ${m}`);
      });
  const archiveRestore = (id: string) =>
    archiveRun(id, api.archiveRestore(id, targetServer()).then((r) => { setView("deck"); refetchTree(); return r; }));
  // Several sessions (a worktree or a whole repo) go one after another; the
  // table refreshes once at the end. The first failure stops the rest.
  const archiveDeleteMany = (plan: { id: string; removeWorktree: boolean }[]) => {
    if (!plan.length) return;
    archiveRun(plan[0].id, (async () => { for (const step of plan) await api.archiveDelete(step.id, step.removeWorktree); })());
  };
  const archiveSummarize = (id: string) => archiveRun(id, api.archiveSummarize(id));
  const [groups, { refetch: refetchGroups, mutate: setGroups }] = createResource<RepoGroup[]>(async () => {
    try { return await api.archiveGroupsGet(); } catch { return []; }
  }, { initialValue: [] });
  const saveGroups = (next: RepoGroup[]) => {
    setGroups(next);
    api.archiveGroupsSet(next).then((saved) => setGroups(saved)).catch((e) => { setArchiveError(String(e)); refetchGroups(); });
  };
  const [harnessError, setHarnessError] = createSignal<string | null>(null);
  // HARNESS: Claude Code config of the archive's repositories (+ ~/.claude); rescanned when the repo set changes.
  const harnessRefs = createMemo(() => JSON.stringify(harnessRepos(archive() ?? [])));
  const [harness, { refetch: refetchHarness }] = createResource(harnessRefs, async (refs) => {
    try { setHarnessError(null); return await api.harnessScan(JSON.parse(refs)); } catch (e) { setHarnessError(String(e)); return []; }
  });

  // QUICKHACKS: pinned harness launchers (Deck's quickhacks.json).
  const [hacksSaved, { mutate: setHacksSaved }] = createResource(() => api.quickhacksGet().catch(() => ({ hacks: [], collapsed: false })));
  const saveHacks = (next: { hacks: Hack[]; collapsed: boolean }) => {
    setHacksSaved(next);
    api.quickhacksSet(next).then((saved) => setHacksSaved(saved)).catch((e) => showError(String(e)));
  };
  const hacksWidth = () => (hacksSaved()?.collapsed ? 30 : hacksWidthRaw());

  // UPLINK: behind/ahead per open checkout (local git, after each tree and
  // every minute), `git fetch` of each repo root every 5 minutes, the pull log.
  const [gitSync, setGitSync] = createSignal<Record<string, GitSync>>({});
  const checkoutPaths = () => [...new Set((tree()?.workspaces ?? []).map((w) => w.worktree?.checkout_path).filter((x): x is string => !!x))];
  const repoRoots = () => [...new Set((tree()?.workspaces ?? []).filter((w) => w.worktree && !w.worktree.is_linked_worktree).map((w) => w.worktree!.checkout_path).filter((x): x is string => !!x))];
  let syncAt = 0;
  const refreshSync = async (force = false) => {
    if (!force && Date.now() - syncAt < 20_000) return;
    syncAt = Date.now();
    const paths = checkoutPaths();
    if (!paths.length) return;
    try { setGitSync(await api.uplinkSync(paths)); } catch { /* git missing or busy: keep the last state */ }
  };
  createEffect(() => { if (tree()) void refreshSync(); });
  onMount(() => {
    const t1 = setInterval(() => void refreshSync(true), 60_000);
    const t2 = setInterval(() => { const r = repoRoots(); if (r.length) api.uplinkFetch(r).then(() => refreshSync(true)).catch(() => {}); }, 5 * 60_000);
    setTimeout(() => { const r = repoRoots(); if (r.length) api.uplinkFetch(r).then(() => refreshSync(true)).catch(() => {}); }, 8_000);
    onCleanup(() => { clearInterval(t1); clearInterval(t2); });
  });
  const [uplink, { refetch: refetchUplink, mutate: setUplink }] = createResource(() => api.uplinkList().then((r) => r.updates).catch(() => []));
  const [uplinkError, setUplinkError] = createSignal<string | null>(null);
  const unseenUpdates = () => (uplink() ?? []).filter((u) => !u.seen).length;
  const pull = async (w: Workspace) => {
    const path = w.worktree?.checkout_path;
    if (!path) { showError("not a git checkout"); return; }
    const repo = w.worktree?.repo_name ?? w.label;
    showError(`pulling ${repo} · ${w.worktree?.branch ?? w.label}…`);
    try {
      const u = await api.uplinkPull(repo, path);
      setUplinkError(null);
      refetchUplink();
      refreshSync(true);
      showError(u.commits.length ? `pulled ${u.commits.length} commit${u.commits.length > 1 ? "s" : ""} into ${repo} · ${u.branch} — see UPLINK` : `${repo} · ${u.branch}: already up to date`);
    } catch (e) { setUplinkError(String(e)); showError(`pull: ${String(e)}`); }
  };
  onMount(() => { let un: (() => void) | undefined; api.onUplink(() => refetchUplink()).then((u) => (un = u)); onCleanup(() => un?.()); });
  createEffect(() => { if (view() === "uplink" && unseenUpdates()) api.uplinkSeen().then((r) => setUplink(r.updates)).catch(() => {}); });

  const [menu, setMenu] = createSignal<MenuState | null>(null);
  // "+ new tab" asks what to run: the tab is created, then the command is typed
  // into its shell once the prompt is up (as ARCHIVE's restore does).
  // The "+" picker: on a space it creates a tab, after OPEN REPOSITORY / NEW
  // WORKTREE it runs in the fresh space's own first pane (`pane`).
  const [newTabFor, setNewTabFor] = createSignal<{ workspaceId: string; pane?: string } | null>(null);
  const newTab = (workspaceId: string, pane?: string) => setNewTabFor({ workspaceId, pane });
  /** Wait for the shell prompt (≤ 6 s), then type `command` + Enter into `paneId`. */
  const startIn = async (paneId: string, command: string) => {
    const t0 = Date.now();
    while (Date.now() - t0 < 6000) {
      await new Promise((res) => setTimeout(res, 300));
      try {
        const { read } = await api.paneRead(paneId, 8);
        const last = read.text.split("\n").map((l) => l.trimEnd()).filter(Boolean).pop() ?? "";
        if (/[>$%❯#]\s*$/.test(last)) break;
      } catch { /* not readable yet */ }
    }
    await api.sendText(paneId, command);
    await api.sendKeys(paneId, ["enter"]);
  };
  const openTab = async (target: { workspaceId: string; pane?: string }, kind: TabKind) => {
    remember("deck.newTabKind", kind.id === "custom" ? "claude" : kind.id);
    try {
      let paneId = target.pane;
      if (!paneId) {
        const r = (await api.action("tab.create", target.workspaceId)) as { root_pane?: { pane_id?: string } } | null;
        refetchTree();
        paneId = r?.root_pane?.pane_id;
      }
      if (!paneId) return;
      setSelectedPane(paneId);
      if (kind.command) await startIn(paneId, kind.command);
    } catch (e) { showError(`tab.create: ${String(e)}`); }
  };
  // "+" next to SESSIONS: open a repository as a space, then ask what to run in it.
  const [repoBox, setRepoBox] = createSignal(false);
  /** The server the "+" was pressed for (a section header), else `targetServer()`. */
  const [repoServer, setRepoServer] = createSignal<string | null>(null);
  const newRepo = (server?: string) => { setRepoServer(server ?? null); setRepoBox(true); };
  const knownRepos = (): KnownRepo[] => {
    const open = new Set((tree()?.workspaces ?? []).map((w) => (w.worktree?.checkout_path ?? "").replace(/^\\\\\?\\/, "").replace(/[\\/]+/g, "/").replace(/\/$/, "").toLowerCase()).filter(Boolean));
    return (harness() ?? []).filter((r) => !r.personal && r.exists).map((r) => ({ name: r.name, root: r.root, open: open.has(r.root.replace(/[\\/]+/g, "/").replace(/\/$/, "").toLowerCase()) }));
  };
  const openRepo = async (root: string) => {
    try {
      const server = repoServer() ?? targetServer();
      if (server === null) { showError("no server connected — ⚙ PREFERENCES"); return; }
      const r = await api.repoOpen(root, undefined, server);
      refetchTree();
      newTab(r.workspace_id, r.pane_id);
    } catch (e) { showError(`open repository: ${String(e)}`); }
  };
  // "+" on a repository row: a new linked worktree, then ask what to run in it.
  const [wtBox, setWtBox] = createSignal<{ repo: string; root: string; server: string | null } | null>(null);
  const createWorktree = async (root: string, branch: string, base: string, server: string | null) => {
    try {
      const target = server ?? targetServer();
      if (target === null) { showError("no server connected — ⚙ PREFERENCES"); return; }
      const r = await api.worktreeCreate(root, branch, base || undefined, target);
      refetchTree();
      newTab(r.workspace_id, r.pane_id);
    } catch (e) { showError(`worktree.create: ${String(e)}`); }
  };
  // RENAME for a space / tab / pane: Deck's prompt → `*.rename {label}`.
  const [ask, setAsk] = createSignal<{ title: string; value: string; ok: (v: string) => void } | null>(null);
  const rename = (kind: "workspace" | "tab" | "pane", id: string, current: string) =>
    setAsk({ title: `RENAME ${kind.toUpperCase()}`, value: current, ok: (label) => { if (label !== current) runAction(`${kind}.rename` as TreeAction, id, { label }); } });
  const [overlay, setOverlay] = createSignal<"palette" | "accounts" | null>(null);
  const overlayOpen = () => menu() !== null || overlay() !== null;
  const closeOverlays = () => { setMenu(null); setOverlay(null); };

  const workspaceMenu = (e: MouseEvent, w: Workspace) => setMenu({
    x: e.clientX, y: e.clientY, title: w.label.toUpperCase(),
    items: [
      { label: "FOCUS IN HERDR", hint: "workspace.focus", run: () => api.focus("workspace", w.workspace_id).catch((er) => showError(String(er))) },
      { label: "RENAME", hint: "workspace.rename", run: () => rename("workspace", w.workspace_id, w.label) },
      ...(w.worktree?.checkout_path ? [{ label: "PULL · UPDATE", hint: "git pull --ff-only · log in UPLINK", run: () => pull(w) }] : []),
      { label: "NEW TAB", hint: "tab.create", run: () => newTab(w.workspace_id) },
      { label: "NEW SPACE", hint: "workspace.create", run: () => runAction("workspace.create", w.workspace_id) },
      { label: "CLOSE · ARCHIVE", hint: "keep for restore", run: () => archiveClose(w) },
      { label: "CLOSE SPACE", hint: "workspace.close", danger: true, run: () => runAction("workspace.close", w.workspace_id) },
      // A primary repo workspace with linked worktrees refuses a plain close (workspace_group_close_required).
      ...(w.worktree ? [] : [
        { label: "CLOSE · ARCHIVE + WORKTREES", hint: "close_group, keep for restore", run: () => archiveClose(w, true) },
        { label: "CLOSE SPACE + WORKTREES", hint: "close_group", danger: true, run: () => runAction("workspace.close", w.workspace_id, { close_group: true }) },
      ]),
    ],
  });
  // The pane a tab-level action applies to: its focused pane, else the first.
  const tabPane = (t: Tab) => { const ps = tree()?.panes.filter((p) => p.tab_id === t.tab_id) ?? []; return ps.find((p) => p.focused) ?? ps[0]; };
  const tabMenu = (e: MouseEvent, t: Tab) => setMenu({
    x: e.clientX, y: e.clientY, title: t.label.toUpperCase(),
    items: [
      { label: "FOCUS IN HERDR", hint: "tab.focus", run: () => api.focus("tab", t.tab_id).catch((er) => showError(String(er))) },
      { label: "RENAME", hint: "tab.rename", run: () => rename("tab", t.tab_id, t.label) },
      { label: "NEW TAB", hint: "tab.create", run: () => newTab(t.workspace_id) },
      { label: "SPLIT RIGHT", hint: "pane.split", run: () => runAction("pane.split", tabPane(t)?.pane_id, { direction: "right" }) },
      { label: "SPLIT DOWN", hint: "pane.split", run: () => runAction("pane.split", tabPane(t)?.pane_id, { direction: "down" }) },
      { label: "CLOSE TAB", hint: "tab.close", danger: true, run: () => runAction("tab.close", t.tab_id) },
    ],
  });
  const paneMenu = (e: MouseEvent, pane: Pane) => setMenu({
    x: e.clientX, y: e.clientY, title: (pane.label || pane.display_agent || pane.agent || pane.title || pane.pane_id).toUpperCase(),
    items: [
      { label: "OPEN", run: () => selectPane(pane.pane_id) },
      { label: "FOCUS IN HERDR", hint: "pane.focus", run: () => api.focus("pane", pane.pane_id).catch((er) => showError(String(er))) },
      { label: "RENAME", hint: "pane.rename", run: () => rename("pane", pane.pane_id, pane.label ?? "") },
      { label: "SPLIT RIGHT", hint: "pane.split", run: () => runAction("pane.split", pane.pane_id, { direction: "right" }) },
      { label: "SPLIT DOWN", hint: "pane.split", run: () => runAction("pane.split", pane.pane_id, { direction: "down" }) },
      { label: "CLOSE PANE", hint: "pane.close", danger: true, run: () => runAction("pane.close", pane.pane_id) },
    ],
  });
  const accountMenu = (e: MouseEvent, a: CswapAccount) => setMenu({
    x: e.clientX, y: e.clientY, title: accountLabel(a).toUpperCase(),
    items: [
      { label: a.active ? "ACTIVE" : "SWITCH", hint: "cswap switch", disabled: a.active || switching() !== null, run: () => switchAccount(a.number) },
      { label: "REFRESH", hint: "cswap list", run: () => refetchAccounts() },
    ],
  });

  const perform = (c: Command) => {
    const d = c.do;
    if (d.kind === "archive") { const w = tree()?.workspaces.find((x) => x.workspace_id === d.id); if (w) archiveClose(w, d.group); }
    else if (d.kind === "view") setView(d.view);
    else if (d.kind === "select") selectPane(d.paneId);
    else if (d.kind === "switch") switchAccount(d.account);
    else if (d.kind === "refresh") refetchAccounts();
    else if (d.action === "tab.create" && d.id) newTab(d.id);
    else if (d.action === "workspace.create") newRepo();
    else runAction(d.action, d.id);
  };
  const paletteItems = createMemo((): PaletteItem[] =>
    commands(tree(), accounts()?.accounts ?? [], currentPane()).map((c) => ({ id: c.id, label: c.label, hint: c.hint, keys: c.keys, danger: c.danger, run: () => perform(c) })));
  const accountItems = createMemo((): PaletteItem[] =>
    (accounts()?.accounts ?? []).map((a) => {
      const u = usageOf(a);
      return { id: `acc:${a.number}`, label: accountLabel(a), hint: `${a.active ? "ACTIVE · " : ""}5h ${u.five ?? "--"}% · 7d ${u.week ?? "--"}%`, keys: a.email, run: () => { if (!a.active) switchAccount(a.number); } };
    }));
  const waiting = () => (tree()?.agents ?? []).filter((a) => a.agent_status === "blocked").length;
  const jump = () => {
    const next = nextBlockedPane(tree(), currentPane());
    if (next) selectPane(next); else showError("no agent is waiting");
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.ctrlKey && !e.altKey && !e.shiftKey && !e.metaKey && e.key === ",") {
      e.preventDefault(); e.stopPropagation();
      if (prefsOpen()) closePrefs(); else openPrefs();
      return;
    }
    // Alt+0 = ALL, Alt+1…9 = the n-th connected server; only with more than one.
    if (e.altKey && !e.ctrlKey && !e.shiftKey && !e.metaKey && /^Digit[0-9]$/.test(e.code) && connected().length > 1 && view() === "deck") {
      const n = Number(e.code.slice(5));
      const s = n === 0 ? "all" : connected()[n - 1]?.id;
      if (s) { e.preventDefault(); e.stopPropagation(); setScope(s); }
      return;
    }
    if (prefsOpen()) return;
    const hk = hotkeyOf(e, overlayOpen());
    if (!hk) return;
    e.preventDefault();
    e.stopPropagation();
    if (hk === "close") closeOverlays();
    else if (hk === "jump") { closeOverlays(); jump(); }
    else if (hk === "palette") { setMenu(null); setOverlay(overlay() === "palette" ? null : "palette"); }
    else if (hk === "accounts") { setMenu(null); setOverlay(overlay() === "accounts" ? null : "accounts"); }
  };

  // Tree events (architecture.md F1–F3) → one `herdr_tree` call 200 ms after
  // the burst ends. The fork's `pane_output_changed` for the pane on screen →
  // `outputTick`, which TerminalView turns into a debounced `pane.read`.
  const refetchTreeSoon = debounceTrailing(() => { refetchTree(); }, 200);
  const [outputTick, setOutputTick] = createSignal(0);
  // Last output time per pane (any pane, not only the one on screen), for
  // ordering the session map by recency. Updated at most once a second per
  // pane so a chatty pane does not re-render the sidebar on every event.
  // Seeded from the Claude transcripts' modified times (real recency, survives
  // a Deck restart) and kept in the webview's storage between reloads.
  const [activity, setActivity] = createSignal<Map<string, number>>(loadActivity());
  // Minute tick for "still active" vs "idle" colouring in the session map.
  const [mapNow, setMapNow] = createSignal(Date.now());
  onMount(() => { const t = setInterval(() => setMapNow(Date.now()), 60_000); onCleanup(() => clearInterval(t)); });
  const bumpActivity = (paneId: string, at: number) => {
    const was = activity().get(paneId) ?? 0;
    if (at - was < 1000) return;
    setActivity((m) => { const n = new Map(m).set(paneId, at); saveActivity(n); return n; });
  };
  // Output events count as activity only for panes without a Claude session:
  // the CLI redraws its status line while idle (timers, token counters), which
  // would keep an idle session "recent". Claude panes take the transcript's
  // mtime below — the time of the last real message, from either side.
  const noteActivity = (paneId: string) => {
    const x = tree()?.panes.find((p) => p.pane_id === paneId);
    if (x?.agent_session?.kind === "id" && x.agent_session.value) return;
    bumpActivity(paneId, Date.now());
  };
  // After each tree: transcript mtimes for the Claude panes, at most every 10 s.
  let mtimesAt = 0;
  createEffect(() => {
    const t = tree();
    if (!t || Date.now() - mtimesAt < 10_000) return;
    mtimesAt = Date.now();
    const bySession = new Map<string, string>();
    for (const x of t.panes) { const sid = x.agent_session?.kind === "id" ? x.agent_session.value : null; if (sid) bySession.set(sid, x.pane_id); }
    if (!bySession.size) return;
    // Each server is asked for its own sessions: the transcripts are on its machine.
    const perServer = new Map<string, string[]>();
    for (const [sid, pane] of bySession) { const s = serverOf(pane); perServer.set(s, [...(perServer.get(s) ?? []), sid]); }
    for (const [server, sids] of perServer) api.transcriptMtimes(sids, server).then((m) => {
      // authoritative for Claude panes: set, not max (an old inflated value must not win)
      setActivity((prev) => {
        const n = new Map(prev);
        let changed = false;
        for (const [sid, ms] of Object.entries(m)) { const pane = bySession.get(sid); if (pane && n.get(pane) !== ms) { n.set(pane, ms); changed = true; } }
        if (changed) saveActivity(n);
        return changed ? n : prev;
      });
    }).catch(() => {});
  });

  // Links in rendered markdown (chat, harness viewer) go to the system
  // browser; the webview never navigates away from Deck.
  const onLinkClick = (e: MouseEvent) => {
    const a = (e.target as HTMLElement | null)?.closest("a[href]") as HTMLAnchorElement | null;
    if (!a) return;
    e.preventDefault();
    const href = a.getAttribute("href") ?? "";
    if (/^https?:\/\//i.test(href)) api.openExternal(href).catch((er) => showError(String(er)));
  };
  onMount(() => { document.addEventListener("click", onLinkClick, true); onCleanup(() => document.removeEventListener("click", onLinkClick, true)); });

  onMount(() => {
    let un1: (() => void) | undefined, un2: (() => void) | undefined;
    api.onEvent((ev, server) => {
      setOffline(null);
      if (server && down()[server]) { setDown((d) => { const n = { ...d }; delete n[server]; return n; }); refetchTree(); }
      const outPane = outputPaneOf(ev);
      if (outPane) noteActivity(outPane);
      const route = routeEvent(ev, currentPane());
      if (route === "tree") refetchTreeSoon();
      else if (route === "tick") setOutputTick((n) => n + 1);
    }).then((u) => (un1 = u));
    api.onOffline((server, m) => {
      if (server) setDown((d) => ({ ...d, [server]: m }));
      // The bottom bar is for "nothing is reachable"; one server down shows in its section.
      if (!server || connected().every((s) => s.id === server || isDown(s.id))) setOffline(m);
    }).then((u) => (un2 = u));
    let unS: (() => void) | undefined;
    const refetchServersSoon = debounceTrailing(() => { refetchServers(); }, 150);
    api.onServers(refetchServersSoon).then((u) => (unS = u));
    onCleanup(() => { unS?.(); refetchServersSoon.cancel(); });
    let un3: (() => void) | undefined;
    api.onArchive(() => refetchArchive()).then((u) => (un3 = u));
    const timer = setInterval(() => { refetchTree(); refetchAccounts(); }, 15000);
    checkUpdate();
    const updTimer = setInterval(checkUpdate, 4 * 3600_000);
    // Capture phase: Deck's keys are decided before xterm's textarea sees them.
    window.addEventListener("keydown", onKey, true);
    onCleanup(() => { un1?.(); un2?.(); un3?.(); clearInterval(timer); clearInterval(updTimer); refetchTreeSoon.cancel(); window.removeEventListener("keydown", onKey, true); clearTimeout(errorTimer); });
  });

  // M4: an agent that starts waiting for input while its pane is not on
  // screen (or the window is in the background) raises an OS notification.
  let lastStatus = new Map<string, string>();
  createEffect(() => {
    const t = fullTree();
    if (!t) return;
    const now = new Map(t.agents.map((a) => [a.pane_id, a.agent_status] as const));
    const onScreen = currentPane();
    const focused = typeof document !== "undefined" && document.hasFocus();
    for (const a of t.agents) {
      const before = lastStatus.get(a.pane_id);
      if (a.agent_status === "blocked" && before !== undefined && before !== "blocked" && (a.pane_id !== onScreen || !focused)) {
        api.notify("Agent is waiting", `${agentName(t, a)} needs your input`).catch(() => {});
      }
    }
    lastStatus = now;
  });

  const activeAccount = () => accounts()?.accounts.find((a) => a.active);
  let deckTab: HTMLButtonElement | undefined;
  const anyDown = () => connected().some((s) => isDown(s.id));
  const scopeBad = () => (shown() === "all" ? anyDown() : isDown(shown()));
  const scopeLabel = () => {
    const s = shown();
    if (s === "all") return `ALL ${connected().filter((x) => !isDown(x.id)).length}/${connected().length}`;
    const name = connected().find((x) => x.id === s)?.name ?? s;
    return isDown(s) ? `${name} · OFFLINE` : name;
  };
  const linkBad = () => !connected().length || connected().every((s) => isDown(s.id));
  const linkLabel = () => {
    const c = connected();
    if (!servers()) return "connecting…";
    if (!c.length) return "NO SERVER";
    if (c.length === 1) return `${c[0].kind} · ${isDown(c[0].id) ? "OFFLINE" : "LINK OK"}`;
    const up = c.filter((s) => !isDown(s.id)).length;
    return `${up}/${c.length} servers · ${up === c.length ? "LINK OK" : up ? "PARTIAL" : "OFFLINE"}`;
  };
  const win = getCurrentWindow();

  // WebView2's own context menu (back, reload, print, inspect…) has nothing
  // for Deck: every surface that has actions opens its own menu, the rest
  // gets none. Text fields keep the browser's cut/copy/paste menu.
  const noBrowserMenu = (e: MouseEvent) => {
    const t = e.target as HTMLElement | null;
    const editable = t?.closest("input, textarea, [contenteditable]") && !t?.closest(".xterm");
    if (!editable) e.preventDefault();
  };

  return (
    <div class="frame" style={{ "grid-template-columns": view() === "deck" ? `${sideWidth()}px 1fr ${hacksWidth()}px` : `${sideWidth()}px 1fr` }} onContextMenu={noBrowserMenu}>
      <Show when={view() === "deck" && !hacksSaved()?.collapsed}><div class="side-grip right" style={{ right: `${hacksWidth() - 3}px` }} onPointerDown={startHacksDrag} onDblClick={() => { setHacksWidthRaw(SIDE_MIN); remember("deck.hacksWidth", null); }} title="drag to resize · double-click resets" /></Show>
      <Show when={view() === "deck"}><div class="side-grip" style={{ left: `${sideWidth() - 3}px` }} onPointerDown={startSideDrag} onDblClick={() => { setSideWidth(SIDE_MIN); remember("deck.sideWidth", null); }} title="drag to resize · double-click resets" /></Show>
      <header class="titlebar" data-tauri-drag-region>
        <div class="vtabs">
          <button ref={deckTab} class={`vtab deck ${view() === "deck" ? "on" : ""}`}
                  aria-haspopup={view() === "deck" ? "menu" : undefined} aria-expanded={view() === "deck" ? srvMenu() : undefined} aria-controls={srvMenu() ? "srv-menu" : undefined}
                  title={view() === "deck" ? "servers shown here (click)" : undefined}
                  onClick={() => (view() === "deck" ? setSrvMenu(!srvMenu()) : setView("deck"))}
                  onKeyDown={(e) => { if (view() === "deck" && (e.key === "ArrowDown" || e.key === "Enter" || e.key === " ")) { e.preventDefault(); setSrvMenu(true); } }}>
            ▞ NETRUNNER DECK
            <Show when={!connected().length && servers()}><span class="scope bad">NO SERVER</span></Show>
            <Show when={connected().length > 1 || shown() !== "all"}><span class={`scope ${scopeBad() ? "bad" : ""}`}>{scopeLabel()}</span></Show>
            <Show when={view() === "deck"}><span class="car">▾</span></Show>
          </button>
          <button class={`vtab ${view() === "harness" ? "on" : ""}`} onClick={() => setView("harness")} title="Claude Code configuration per repository">▞ HARNESS</button>
          <button class={`vtab ${view() === "uplink" ? "on" : ""}`} onClick={() => setView("uplink")} title="what git pull brought in">▞ UPLINK{unseenUpdates() ? ` · ${unseenUpdates()}` : ""}</button>
          <button class={`vtab ${view() === "archive" ? "on" : ""}`} onClick={() => setView("archive")} title="closed spaces kept for restore">▞ ARCHIVE{(archive() ?? []).length ? ` · ${(archive() ?? []).filter((e) => e.status === "closed").length}` : ""}</button>
        </div>
        <span class={`micro link ${linkBad() ? "bad" : ""}`} title={connected().map((s) => `${s.name}: ${isDown(s.id) ? down()[s.id] ?? s.status.error ?? "offline" : s.status.state}`).join("\n")}>{linkLabel()}</span>
        <Show when={actionError()}>{(m) => <span class="micro toast" title={m()}>✕ {m()}</span>}</Show>
        <Show when={update()}>{(u) => (
          <button class="micro upd" disabled={installing()} title={u().notes ?? ""} onClick={installUpdate}>
            {installing() ? "⇣ INSTALLING…" : `⇣ UPDATE v${u().version} — INSTALL`}
          </button>
        )}</Show>
        <span class="sp" />
        <Pulse tree={tree()} />
        <Show when={activeAccount()} fallback={<div class="zone danger"><i />NO LINK</div>}>
          {(a) => {
            const u = usageOf(a());
            // The colour is the quota zone (safe < 50 % < public < 80 % < restricted < 100 % = danger); the text is the numbers.
            return (
              <div class={`zone ${zoneOf(a())}`} title="active account · 5-hour and 7-day quota used">
                <i />{accountLabel(a())}<span class="pct">5h {u.five ?? "--"}%</span><span class="pct">7d {u.week ?? "--"}%</span>
              </div>
            );
          }}
        </Show>
        <button class="gear" title="Preferences (Ctrl+,)" aria-label={`Preferences${anyDown() ? " — a server is unreachable" : ""}${offered().some((s) => s.new) ? " — new server found" : ""}`} onClick={() => (prefsOpen() ? closePrefs() : openPrefs())}>
          <span aria-hidden="true">⚙</span>
          <Show when={anyDown()}><i class="pip bad" /></Show>
          <Show when={!anyDown() && offered().some((s) => s.new)}><i class="pip new" /></Show>
        </button>
        <Clock />
        <div class="win">
          <button onClick={() => win.minimize()}>—</button>
          <button onClick={() => win.toggleMaximize()}>▢</button>
          <button onClick={() => win.close()}>✕</button>
        </div>
      </header>

      <Show when={view() === "harness"}>
        <HarnessView repos={harness()} error={harnessError()} groups={groups()} onGroups={saveGroups} onRefresh={() => refetchHarness()} />
      </Show>
      <Show when={view() === "uplink"}>
        <UplinkView updates={uplink()} error={uplinkError()} onRemove={(id) => api.uplinkRemove(id).then((r) => setUplink(r.updates)).catch((e) => setUplinkError(String(e)))} onRefresh={() => refetchUplink()} />
      </Show>
      <Show when={view() === "archive"}>
        <ArchiveView entries={archive()} busy={archiveBusy()} error={archiveError()} onRestore={archiveRestore} onDeleteMany={archiveDeleteMany} onSummarize={archiveSummarize} onRefresh={() => refetchArchive()} groups={groups()} onGroups={saveGroups} />
      </Show>
      <Show when={view() === "deck"}>
      <Sidebar
        tree={tree()}
        accounts={accounts()}
        accountsError={accountsError()}
        switching={switching()}
        switchError={switchError()}
        selectedPane={currentPane()}
        activity={activity()}
        now={mapNow()}
        gitSync={gitSync()}
        onSelectPane={selectPane}
        onSwitchAccount={switchAccount}
        onRefreshAccounts={() => refetchAccounts()}
        onWorkspaceMenu={workspaceMenu}
        onTabMenu={tabMenu}
        onPaneMenu={paneMenu}
        onAccountMenu={accountMenu}
        onNewRepo={newRepo}
        onNewWorktree={(repo, root, server) => setWtBox({ repo, root, server: server ?? null })}
        sections={sections()}
        empty={servers() && !connected().length ? () => (
          <div class="first-run">
            <div class="plate">Connect a server</div>
            <For each={offered()} fallback={<div class="row micro">{servers()?.scanned ? "No servers found. Start Herdr or deck-server." : "looking for servers…"}</div>}>{(s) => (
              <div class="row" title={s.status.error ?? s.address} onClick={() => connectServer(s.id)}>
                <span class={`dot ${dotOf(s)}`} />
                <span class="nm">{s.name}</span>
                <span class="kind">{s.kind.toUpperCase()}</span>
                <button class="btn pri" onClick={(e) => { e.stopPropagation(); connectServer(s.id); }}>CONNECT</button>
              </div>
            )}</For>
            <button class="btn" onClick={openPrefs}>⚙ PREFERENCES</button>
          </div>
        ) : undefined}
        onNewTab={newTab}
        onSplit={(paneId) => runAction("pane.split", paneId, { direction: "right" })}
      />

      <main class="main">
        <TabBar tree={tree()} selectedPane={currentPane()} onSelectPane={setSelectedPane} onTabMenu={tabMenu} onNewTab={newTab} />
        <Show when={currentPane()} fallback={<div class="empty">NO PANE SELECTED · open a workspace in Herdr</div>}>
          {(id) => (
            <Show when={!termPanes().has(id()) && claudeSession(id())}
              fallback={<TerminalView paneId={id()} tree={tree()} outputTick={outputTick()} onMenu={setMenu} chat={claudeSession(id()) ? () => setPaneView(id(), "chat") : undefined} />}>
              {(sid) => <ChatView paneId={id()} sessionId={sid()} tree={tree()} outputTick={outputTick()} harness={harness()} onTerminal={() => setPaneView(id(), "term")} />}
            </Show>
          )}
        </Show>
        <Show when={offline() && connected().length ? offline() : null}>{(m) => <div class="offline">◇ {connected().length > 1 ? "NO SERVER REACHABLE" : "HERDR OFFLINE"} — {m()}</div>}</Show>
      </main>
      <QuickHacks hacks={hacksSaved()?.hacks ?? []} collapsed={hacksSaved()?.collapsed ?? false} harness={harness()}
        onSave={(hacks) => saveHacks({ hacks, collapsed: hacksSaved()?.collapsed ?? false })}
        onCollapse={(collapsed) => saveHacks({ hacks: hacksSaved()?.hacks ?? [], collapsed })}
        onOpenPane={(id) => { setSelectedPane(id); refetchTree(); }} onError={showError} server={targetServer()} />
      </Show>

      <KeyBar tree={tree()} selectedPane={currentPane()} account={activeAccount()} waiting={waiting()} />

      <Show when={menu()}>{(m) => <ContextMenu menu={m()} onClose={() => setMenu(null)} />}</Show>
      <Show when={srvMenu() && view() === "deck"}>
        <div class="srv-anchor" style={{ left: `${deckTab?.getBoundingClientRect().left ?? 0}px`, top: `${deckTab?.getBoundingClientRect().bottom ?? 40}px` }}>
          <ServerMenu connected={connected()} offered={offered()} scope={shown()} panes={panesOf}
            onScope={setScope} onConnect={connectServer} onManage={openPrefs} onClose={() => { setSrvMenu(false); deckTab?.focus(); }} />
        </div>
      </Show>
      <Show when={prefsOpen()}>
        <Prefs view={servers()} panes={panesOf} scanning={scanning()} error={prefsError()} onScan={scanServers}
          onConnect={connectServer} onDisconnect={(id) => serverCall(api.serverDisconnect(id))}
          onRename={(id, name) => serverCall(api.serverRename(id, name))}
          onForget={(id) => serverCall(api.serverForget(id))} onRemoveHost={(m) => serverCall(api.serverRemoveHost(m))}
          onProbe={api.serverProbe} onAddHost={(u, h, port, sockets) => api.serverAddHost(u, h, port, sockets).then((v) => { setServers(v); setPrefsError(null); })}
          onClose={closePrefs} />
      </Show>
      <Show when={newTabFor()}>{(t) => { const target = t(); return <NewTabBox initial={remembered("deck.newTabKind") ?? "claude"} onPick={(k) => openTab(target, k)} onClose={() => setNewTabFor(null)} />; }}</Show>
      <Show when={repoBox()}><NewRepoBox known={knownRepos()} onPick={openRepo} onClose={() => setRepoBox(false)} /></Show>
      <Show when={wtBox()}>{(w) => { const t = w(); return <NewWorktreeBox repo={t.repo} onCreate={(b, base) => createWorktree(t.root, b, base, t.server)} onClose={() => setWtBox(null)} />; }}</Show>
      <Show when={ask()}>{(q) => <PromptBox title={q().title} value={q().value} placeholder="name" ok={q().ok} onClose={() => setAsk(null)} />}</Show>
      <Show when={overlay() === "palette"}><Palette title="Palette" items={paletteItems()} onClose={() => setOverlay(null)} /></Show>
      <Show when={overlay() === "accounts"}><Palette title="Account" items={accountItems()} digits onClose={() => setOverlay(null)} /></Show>
    </div>
  );
}
