import { For, createMemo } from "solid-js";
import type { Tree } from "../api";

export default function TabBar(p: { tree?: Tree; selectedPane: string | null; onSelectPane: (id: string) => void }) {
  const tabs = createMemo(() => {
    const t = p.tree;
    if (!t || !p.selectedPane) return [];
    const pane = t.panes.find((x) => x.pane_id === p.selectedPane);
    if (!pane) return [];
    return t.tabs
      .filter((tab) => tab.workspace_id === pane.workspace_id)
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
          <div class={`tab ${t.on ? "on" : ""}`} onClick={() => t.pane && p.onSelectPane(t.pane.pane_id)}>
            {t.tab.agent_status === "working" ? "◆" : t.tab.agent_status === "blocked" ? "◈" : "◇"} {t.label.toUpperCase()}
          </div>
        )}
      </For>
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
  let el!: HTMLSpanElement;
  setInterval(() => { if (el) el.textContent = fmt(); }, 10000);
  return <span class="d" ref={el}>{fmt()}</span>;
}
