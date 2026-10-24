import { createResource, createSignal, onCleanup, onMount, Show } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, type Tree, type CswapList, zoneOf, usageOf, accountLabel } from "./api";
import Sidebar, { type SwitchError } from "./components/Sidebar";
import TabBar from "./components/TabBar";
import TerminalView from "./components/TerminalView";
import KeyBar from "./components/KeyBar";
import { createLatestGate } from "./latest";
import { debounceTrailing, routeEvent } from "./events";

export default function App() {
  const [selectedPane, setSelectedPane] = createSignal<string | null>(null);
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
    onCleanup(() => { un1?.(); un2?.(); clearInterval(timer); refetchTreeSoon.cancel(); });
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
        accountsError={accountsError()}
        switching={switching()}
        switchError={switchError()}
        selectedPane={currentPane()}
        onSelectPane={(id) => { setSelectedPane(id); api.focus("pane", id).catch(() => {}); }}
        onSwitchAccount={switchAccount}
        onRefreshAccounts={() => refetchAccounts()}
      />

      <main class="main">
        <TabBar tree={tree()} selectedPane={currentPane()} onSelectPane={setSelectedPane} />
        <Show when={currentPane()} fallback={<div class="empty">NO PANE SELECTED · open a workspace in Herdr</div>}>
          {(id) => <TerminalView paneId={id()} tree={tree()} outputTick={outputTick()} />}
        </Show>
        <Show when={offline()}>{(m) => <div class="offline">◇ HERDR OFFLINE — {m()}</div>}</Show>
      </main>

      <KeyBar tree={tree()} selectedPane={currentPane()} account={activeAccount()} />
    </div>
  );
}
