import { createEffect, createMemo, createResource, createSignal, onCleanup, onMount, Show } from "solid-js";
import { getCurrentWindow } from "./tauri";
import { api, type Tree, type CswapList, type Tab, type Workspace, type Agent, type CswapAccount, type TreeAction, zoneOf, usageOf, accountLabel } from "./api";
import Sidebar, { type SwitchError } from "./components/Sidebar";
import TabBar from "./components/TabBar";
import TerminalView from "./components/TerminalView";
import KeyBar from "./components/KeyBar";
import ContextMenu, { type MenuState } from "./components/ContextMenu";
import Palette, { type PaletteItem } from "./components/Palette";
import { createLatestGate } from "./latest";
import { debounceTrailing, routeEvent } from "./events";
import { agentName, commands, hotkeyOf, nextBlockedPane, type Command } from "./actions";

/** UI-only preferences in the webview's storage; never session state (that lives in Herdr). */
function remembered(key: string): string | null {
  try { return localStorage.getItem(key); } catch { return null; }
}
function remember(key: string, value: string | null) {
  try { if (value === null) localStorage.removeItem(key); else localStorage.setItem(key, value); } catch { /* storage unavailable */ }
}

export default function App() {
  const [selectedPane, setSelectedPaneRaw] = createSignal<string | null>(remembered("deck.selectedPane"));
  const setSelectedPane = (id: string | null) => { setSelectedPaneRaw(id); remember("deck.selectedPane", id); };
  const [offline, setOffline] = createSignal<string | null>(null);
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
  const [tree, { refetch: refetchTree }] = createResource<Tree | undefined>(async (_, { value }) => {
    const seq = gate.begin("tree");
    try {
      const t = await api.tree();
      if (!gate.isCurrent("tree", seq)) return value;
      setOffline(null);
      return t;
    } catch (e) {
      if (!gate.isCurrent("tree", seq)) return value;
      setOffline(String(e));
      return value;
    }
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
  const runAction = (action: TreeAction, id?: string | null, options?: { cwd?: string; label?: string; close_group?: boolean }) =>
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

  const [menu, setMenu] = createSignal<MenuState | null>(null);
  const [overlay, setOverlay] = createSignal<"palette" | "accounts" | null>(null);
  const overlayOpen = () => menu() !== null || overlay() !== null;
  const closeOverlays = () => { setMenu(null); setOverlay(null); };

  const workspaceMenu = (e: MouseEvent, w: Workspace) => setMenu({
    x: e.clientX, y: e.clientY, title: w.label.toUpperCase(),
    items: [
      { label: "FOCUS IN HERDR", hint: "workspace.focus", run: () => api.focus("workspace", w.workspace_id).catch((er) => showError(String(er))) },
      { label: "NEW TAB", hint: "tab.create", run: () => runAction("tab.create", w.workspace_id) },
      { label: "NEW SPACE", hint: "workspace.create", run: () => runAction("workspace.create", w.workspace_id) },
      { label: "CLOSE SPACE", hint: "workspace.close", danger: true, run: () => runAction("workspace.close", w.workspace_id) },
      // A primary repo workspace with linked worktrees refuses a plain close (workspace_group_close_required).
      ...(w.worktree ? [] : [{ label: "CLOSE SPACE + WORKTREES", hint: "close_group", danger: true, run: () => runAction("workspace.close", w.workspace_id, { close_group: true }) }]),
    ],
  });
  const tabMenu = (e: MouseEvent, t: Tab) => setMenu({
    x: e.clientX, y: e.clientY, title: t.label.toUpperCase(),
    items: [
      { label: "FOCUS IN HERDR", hint: "tab.focus", run: () => api.focus("tab", t.tab_id).catch((er) => showError(String(er))) },
      { label: "NEW TAB", hint: "tab.create", run: () => runAction("tab.create", t.workspace_id) },
      { label: "CLOSE TAB", hint: "tab.close", danger: true, run: () => runAction("tab.close", t.tab_id) },
    ],
  });
  const agentMenu = (e: MouseEvent, a: Agent) => setMenu({
    x: e.clientX, y: e.clientY, title: agentName(tree(), a).toUpperCase(),
    items: [
      { label: "OPEN", run: () => selectPane(a.pane_id) },
      { label: "FOCUS IN HERDR", hint: "pane.focus", run: () => api.focus("pane", a.pane_id).catch((er) => showError(String(er))) },
      { label: "CLOSE PANE", hint: "pane.close", danger: true, run: () => runAction("pane.close", a.pane_id) },
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
    if (d.kind === "select") selectPane(d.paneId);
    else if (d.kind === "switch") switchAccount(d.account);
    else if (d.kind === "refresh") refetchAccounts();
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

  onMount(() => {
    let un1: (() => void) | undefined, un2: (() => void) | undefined;
    api.onEvent((ev) => {
      setOffline(null);
      const route = routeEvent(ev, currentPane());
      if (route === "tree") refetchTreeSoon();
      else if (route === "tick") setOutputTick((n) => n + 1);
    }).then((u) => (un1 = u));
    api.onOffline((m) => setOffline(m)).then((u) => (un2 = u));
    const timer = setInterval(() => { refetchTree(); refetchAccounts(); }, 15000);
    // Capture phase: Deck's keys are decided before xterm's textarea sees them.
    window.addEventListener("keydown", onKey, true);
    onCleanup(() => { un1?.(); un2?.(); clearInterval(timer); refetchTreeSoon.cancel(); window.removeEventListener("keydown", onKey, true); clearTimeout(errorTimer); });
  });

  // M4: an agent that starts waiting for input while its pane is not on
  // screen (or the window is in the background) raises an OS notification.
  let lastStatus = new Map<string, string>();
  createEffect(() => {
    const t = tree();
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
  const win = getCurrentWindow();

  return (
    <div class="frame">
      <header class="titlebar" data-tauri-drag-region>
        <div class="logo">▞ NETRUNNER DECK</div>
        <span class="micro">herdr client · {offline() ? "OFFLINE" : "LINK OK"}</span>
        <Show when={actionError()}>{(m) => <span class="micro toast" title={m()}>✕ {m()}</span>}</Show>
        <span class="sp" />
        <Show when={activeAccount()} fallback={<div class="zone danger"><i />ZONE : NO LINK</div>}>
          {(a) => {
            const z = zoneOf(a());
            const u = usageOf(a());
            return (
              <div class={`zone ${z}`}>
                <i />ZONE : {z.toUpperCase()}
                <span class="micro">{accountLabel(a())} · 5h {u.five ?? "--"}% · 7d {u.week ?? "--"}%</span>
              </div>
            );
          }}
        </Show>
        <div class="win">
          <button onClick={() => win.minimize()}>—</button>
          <button onClick={() => win.toggleMaximize()}>▢</button>
          <button onClick={() => win.close()}>✕</button>
        </div>
      </header>

      <Sidebar
        tree={tree()}
        accounts={accounts()}
        accountsError={accountsError()}
        switching={switching()}
        switchError={switchError()}
        selectedPane={currentPane()}
        onSelectPane={selectPane}
        onSwitchAccount={switchAccount}
        onRefreshAccounts={() => refetchAccounts()}
        onWorkspaceMenu={workspaceMenu}
        onAgentMenu={agentMenu}
        onAccountMenu={accountMenu}
        onNewSpace={() => runAction("workspace.create", tree()?.panes.find((p) => p.pane_id === currentPane())?.workspace_id)}
      />

      <main class="main">
        <TabBar tree={tree()} selectedPane={currentPane()} onSelectPane={setSelectedPane} onTabMenu={tabMenu} onNewTab={(ws) => runAction("tab.create", ws)} />
        <Show when={currentPane()} fallback={<div class="empty">NO PANE SELECTED · open a workspace in Herdr</div>}>
          {(id) => <TerminalView paneId={id()} tree={tree()} outputTick={outputTick()} />}
        </Show>
        <Show when={offline()}>{(m) => <div class="offline">◇ HERDR OFFLINE — {m()}</div>}</Show>
      </main>

      <KeyBar tree={tree()} selectedPane={currentPane()} account={activeAccount()} waiting={waiting()} />

      <Show when={menu()}>{(m) => <ContextMenu menu={m()} onClose={() => setMenu(null)} />}</Show>
      <Show when={overlay() === "palette"}><Palette title="Palette" items={paletteItems()} onClose={() => setOverlay(null)} /></Show>
      <Show when={overlay() === "accounts"}><Palette title="Account" items={accountItems()} digits onClose={() => setOverlay(null)} /></Show>
    </div>
  );
}
