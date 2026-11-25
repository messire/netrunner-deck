import type { Account, Tree } from "../api";
import { localId } from "../servers";
import { accountLabel } from "../api";
import { buildLine } from "../build";

/** Key hints: only keys Deck really handles (src/actions.ts), plus the mouse actions; the build on the right. */
export default function KeyBar(p: { tree?: Tree; selectedPane: string | null; account?: Account; waiting: number; server?: { name: string; kind: string; version: string | null } }) {
  const where = () => {
    const pane = p.tree?.panes.find((x) => x.pane_id === p.selectedPane);
    return pane ? `${localId(pane.workspace_id)} · ${localId(pane.tab_id)} · ${localId(pane.pane_id)}` : "—";
  };
  return (
    <footer class="keys">
      <span><b>F1</b>ACCOUNT</span>
      <span class={p.waiting ? "o" : ""}><b>F2</b>WAITING{p.waiting ? ` · ${p.waiting}` : ""}</span>
      <span><b>CTRL+K</b>PALETTE</span>
      <span><b>R-CLICK</b>SPACE · TAB · AGENT</span>
      <div class="right"><span>{where()}</span><span class="d">◆ {p.account ? accountLabel(p.account) : "NO LINK"}</span>
        <span class="build" title={`Netrunner Deck ${__DECK_VERSION__}, commit ${__DECK_COMMIT__}, built ${new Date(__DECK_BUILT__).toLocaleString()}${p.server ? `
${p.server.name}: ${p.server.kind} ${p.server.version ?? "?"}` : ""}`}>
          {buildLine(__DECK_VERSION__, __DECK_COMMIT__, import.meta.env.DEV)}{p.server?.version ? ` · ${p.server.kind} ${p.server.version}` : ""}
        </span></div>
    </footer>
  );
}
