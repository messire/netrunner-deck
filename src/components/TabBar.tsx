import { For, createMemo, createSignal, onCleanup } from "solid-js";
import type { Tab, Tree } from "../api";

export default function TabBar(p: {
  tree?: Tree;
  selectedPane: string | null;
  onSelectPane: (id: string) => void;
  onTabMenu: (e: MouseEvent, tab: Tab) => void;
  onNewTab: (workspaceId: string) => void;
}) {
  const workspaceId = () => p.tree?.panes.find((x) => x.pane_id === p.selectedPane)?.workspace_id ?? null;
  const tabs = createMemo(() => {
    const t = p.tree;
    const ws = workspaceId();
    if (!t || !ws) return [];
    return t.tabs
      .filter((tab) => tab.workspace_id === ws)
      .map((tab) => {
        const panes = t.panes.filter((x) => x.tab_id === tab.tab_id);
        const agent = panes.find((x) => x.agent);
        const label = agent ? `${(agent.display_agent ?? agent.agent ?? "").toUpperCase()} · ${tab.label}` : tab.label;
        return { tab, pane: agent ?? panes[0], label, on: panes.some((x) => x.pane_id === p.selectedPane) };
      });
  });
  const pulse = createMemo(() => {
    const agents = p.tree?.agents ?? [];
    const order: Record<string, number> = { working: 0, blocked: 1 };
    const glyph: Record<string, string> = { working: "◆", blocked: "◈" };
    const states = agents.map((a) => a.agent_status).sort((a, b) => (order[a] ?? 2) - (order[b] ?? 2));
    const busy = states.filter((s) => s === "working").length;
    return `${states.slice(0, 8).map((s) => glyph[s] ?? "◇").join("")} ${busy}/${states.length}`;
  });
  return (
    <div class="tabs">
      <For each={tabs()}>
        {(t) => (
          <div class={`tab ${t.on ? "on" : ""}`} onClick={() => t.pane && p.onSelectPane(t.pane.pane_id)}
               onContextMenu={(e) => { e.preventDefault(); p.onTabMenu(e, t.tab); }} title="right-click: tab actions">
            {t.tab.agent_status === "working" ? "◆" : t.tab.agent_status === "blocked" ? "◈" : "◇"} {t.label.toUpperCase()}
          </div>
        )}
      </For>
      {workspaceId() ? <div class="tab add" title="new tab in this space (tab.create)" onClick={() => p.onNewTab(workspaceId()!)}>+</div> : null}
      <div class="right">
        <span class="r">{pulse()}</span><span class="m">▕</span>
        <span class="glitch" data-text="▞ AFTER LIFE ▚">▞ AFTER LIFE ▚</span><span class="m">▕</span>
        <Clock />
      </div>
    </div>
  );
}

function Clock() {
  const fmt = () => new Date().toTimeString().slice(0, 5);
  const [now, setNow] = createSignal(fmt());
  const timer = setInterval(() => setNow(fmt()), 10000);
  onCleanup(() => clearInterval(timer));
  return <span class="d">{now()}</span>;
}
