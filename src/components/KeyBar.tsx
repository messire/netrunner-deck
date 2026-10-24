import type { CswapAccount, Tree } from "../api";
import { accountLabel } from "../api";

export default function KeyBar(p: { tree?: Tree; selectedPane: string | null; account?: CswapAccount }) {
  const where = () => {
    const pane = p.tree?.panes.find((x) => x.pane_id === p.selectedPane);
    return pane ? `${pane.workspace_id} · ${pane.tab_id} · ${pane.pane_id}` : "—";
  };
  return (
    <footer class="keys">
      {/* TODO(M3, docs/plan/ROADMAP.md): hotkeys — F1 account picker, F2 jump to
          waiting agent, Ctrl+K palette, Esc. Until then the terminal owns every
          key press, so only the mouse actions that exist today are advertised (R16). */}
      <span><b>DBL-CLICK</b>ACCOUNT · SWITCH</span><span><b>CLICK</b>AGENT · OPEN</span>
      <div class="right"><span>{where()}</span><span class="d">◆ {p.account ? accountLabel(p.account) : "NO LINK"}</span></div>
    </footer>
  );
}
