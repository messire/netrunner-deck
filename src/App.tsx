import { createResource, createSignal, onCleanup, onMount, Show } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, type Tree, type CswapList, zoneOf, usageOf, accountLabel } from "./api";
import Sidebar from "./components/Sidebar";
import TabBar from "./components/TabBar";
import TerminalView from "./components/TerminalView";
import KeyBar from "./components/KeyBar";

export default function App() {
  const [tree, { refetch: refetchTree }] = createResource<Tree>(() => api.tree());
  const [accounts, { refetch: refetchAccounts }] = createResource<CswapList>(() => api.cswapList());
  const [selectedPane, setSelectedPane] = createSignal<string | null>(null);
  const [offline, setOffline] = createSignal<string | null>(null);
  const [tick, setTick] = createSignal(0);

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

  onMount(() => {
    let un1: (() => void) | undefined, un2: (() => void) | undefined;
    api.onEvent((ev) => {
      setOffline(null);
      // Any structural event re-reads the tree; pane output bumps the terminal tick.
      const kind = String(ev.event ?? ev.type ?? "");
      if (kind.startsWith("pane.updated")) setTick((n) => n + 1);
      else refetchTree();
    }).then((u) => (un1 = u));
    api.onOffline((m) => setOffline(m)).then((u) => (un2 = u));
    const timer = setInterval(() => { refetchTree(); refetchAccounts(); }, 15000);
    onCleanup(() => { un1?.(); un2?.(); clearInterval(timer); });
  });

  const activeAccount = () => accounts()?.accounts.find((a) => a.active);
  const win = getCurrentWindow();

  return (
    <div class="frame">
      <header class="titlebar" data-tauri-drag-region>
        <div class="logo">▞ NETRUNNER DECK</div>
        <span class="micro">herdr client · {offline() ? "OFFLINE" : "LINK OK"}</span>
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
        selectedPane={currentPane()}
        onSelectPane={(id) => { setSelectedPane(id); api.focus("pane", id).catch(() => {}); }}
        onSwitchAccount={(n) => api.cswapSwitch(n).then(() => refetchAccounts())}
        onRefreshAccounts={() => refetchAccounts()}
      />

      <main class="main">
        <TabBar tree={tree()} selectedPane={currentPane()} onSelectPane={setSelectedPane} />
        <Show when={currentPane()} fallback={<div class="empty">NO PANE SELECTED · open a workspace in Herdr</div>}>
          {(id) => <TerminalView paneId={id()} tick={tick()} tree={tree()} />}
        </Show>
        <Show when={offline()}>{(m) => <div class="offline">◇ HERDR OFFLINE — {m()}</div>}</Show>
      </main>

      <KeyBar tree={tree()} selectedPane={currentPane()} account={activeAccount()} />
    </div>
  );
}
