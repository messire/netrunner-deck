import type { CswapAccount, Tree } from "../api";
import { accountLabel } from "../api";

/** Key hints: only keys Deck really handles (src/actions.ts), plus the mouse actions. */
export default function KeyBar(p: { tree?: Tree; selectedPane: string | null; account?: CswapAccount; waiting: number }) {
  const where = () => {
    const pane = p.tree?.panes.find((x) => x.pane_id === p.selectedPane);
    return pane ? `${pane.workspace_id} · ${pane.tab_id} · ${pane.pane_id}` : "—";
  };
  return (
    <footer class="keys">
      <span><b>F1</b>ACCOUNT</span>
      <span class={p.waiting ? "o" : ""}><b>F2</b>WAITING{p.waiting ? ` · ${p.waiting}` : ""}</span>
      <span><b>CTRL+K</b>PALETTE</span>
      <span><b>R-CLICK</b>SPACE · TAB · AGENT</span>
      <div class="right"><span>{where()}</span><span class="d">◆ {p.account ? accountLabel(p.account) : "NO LINK"}</span></div>
    </footer>
  );
}
