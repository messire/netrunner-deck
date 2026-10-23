import type { CswapAccount, Tree } from "../api";
import { accountLabel } from "../api";

export default function KeyBar(p: { tree?: Tree; selectedPane: string | null; account?: CswapAccount }) {
  const where = () => {
    const pane = p.tree?.panes.find((x) => x.pane_id === p.selectedPane);
    return pane ? `${pane.workspace_id} · ${pane.tab_id} · ${pane.pane_id}` : "—";
  };
  return (
    <footer class="keys">
      <span><b>F1</b>SWITCH ACCOUNT</span><span><b>F2</b>JUMP TO WAITING</span><span><b>CTRL+K</b>COMMAND</span><span><b>ESC</b>BACK</span>
      <div class="right"><span>{where()}</span><span class="d">◆ {p.account ? accountLabel(p.account) : "NO LINK"}</span></div>
    </footer>
  );
}
