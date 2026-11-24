import { For, Show, createMemo, createSignal } from "solid-js";
import { serverOf } from "../servers";
import { accountLabel, usageOf, type AgentStatus, type Account, type AccountList, type GitSync, type Pane, type Tab, type Tree, type Workspace } from "../api";
import {
  buildTree, ctxPct, defaultOpen, firstPane, loadOpen, saveOpen, waiting,
  type RepoNode, type RootNode, type SessionNode, type SpaceNode, type TabNode,
} from "../tree";

export interface SwitchError { number: number; message: string }

/** A server's part of the tree; the tree is split into these when more than one server is shown. */
export interface SidebarSection {
  id: string;
  name: string;
  /** Online, or why not (the section turns red, its tree dims). */
  error?: string | null;
  connecting?: boolean;
  tree?: Tree;
}

interface Props {
  tree?: Tree;
  accounts?: AccountList;
  /** Why the server's account list failed; shown in the Accounts block, the rest of the UI stays alive. */
  accountsError?: string | null;
  /** Name of the server (machine) whose accounts these are. */
  accountsServer?: string;
  /** "+": sign in to another account on that machine. */
  onAddAccount?: () => void;
  /** Account number of the switch in flight, null/undefined when idle. */
  switching?: number | null;
  switchError?: SwitchError | null;
  selectedPane: string | null;
  onSelectPane: (paneId: string) => void;
  onSwitchAccount: (number: number) => void;
  onRefreshAccounts: () => void;
  /** Last message/output time per pane (ms); orders the session map, newest first. */
  activity?: Map<string, number>;
  /** Clock for the map's active/idle colouring (ticks once a minute). */
  now?: number;
  /** Git behind/ahead per checkout path (UPLINK). */
  gitSync?: Record<string, GitSync>;
  /** Right-click menus; App owns the actions. */
  onWorkspaceMenu: (e: MouseEvent, w: Workspace) => void;
  onTabMenu: (e: MouseEvent, t: Tab) => void;
  onPaneMenu: (e: MouseEvent, pane: Pane) => void;
  onAccountMenu: (e: MouseEvent, a: Account) => void;
  /** Create at any level, like Herdr: space (optionally cloned from one), tab, split. */
  /** "+" next to SESSIONS (or on a server section): open a repository as a new space. */
  onNewRepo: (server?: string) => void;
  /** More than one server shown: one section each. Empty or one = the plain tree. */
  sections?: SidebarSection[];
  /** No server connected at all: the first-run / empty state instead of the tree. */
  empty?: () => unknown;
  /** "+" on a repository row: a new linked worktree inside it (`root` = its main checkout). */
  onNewWorktree: (repoLabel: string, root: string, server?: string) => void;
  /** "+" on a space (worktree) row: a new tab/session in it. */
  onNewTab: (workspaceId: string) => void;
  onSplit: (paneId: string) => void;
}

type Node = RootNode | TabNode;

const statusClass = (s: AgentStatus) => (s === "working" ? "w" : s === "blocked" ? "b" : s === "done" ? "d" : "");
// Only the words the dot cannot carry: working/idle are the dot alone.
const statusWord = (s: AgentStatus, shell: boolean) =>
  shell ? "SHELL" : s === "blocked" ? "WAITING" : s === "done" ? "DONE" : "";
const storage = () => (typeof localStorage === "undefined" ? undefined : localStorage);

/**
 * Sidebar (M6): one tree — repo › space › tab › session — instead of the old
 * Spaces list plus a status-sorted Agents list. Herdr's server order is kept at
 * every level; urgency is a pinned WAITING strip above the tree, not a sort.
 */
export default function Sidebar(p: Props) {
  // Folded sections (a click on the plate toggles; remembered per section).
  const folded = (key: string) => { try { return localStorage.getItem(`deck.${key}Folded`) === "1"; } catch { return false; } };
  const remember = (key: string, v: boolean) => { try { localStorage.setItem(`deck.${key}Folded`, v ? "1" : "0"); } catch { /* storage unavailable */ } };
  const [accountsFolded, setAccountsFoldedRaw] = createSignal(folded("accounts"));
  const setAccountsFolded = (v: boolean) => { setAccountsFoldedRaw(v); remember("accounts", v); };
  const [sessionsFolded, setSessionsFoldedRaw] = createSignal(folded("sessions"));
  const setSessionsFolded = (v: boolean) => { setSessionsFoldedRaw(v); remember("sessions", v); };
  const [mapFolded, setMapFoldedRaw] = createSignal(folded("map"));
  const setMapFolded = (v: boolean) => { setMapFoldedRaw(v); remember("map", v); };
  const roots = createMemo(() => buildTree(p.tree));
  const normPath = (x: string) => x.replace(/^\\\\\?\\/, "").replace(/[\\/]+/g, "/").replace(/\/$/, "").toLowerCase();
  const gitOf = (s: SpaceNode): GitSync | undefined => {
    const path = s.workspace.worktree?.checkout_path;
    if (!path || !p.gitSync) return undefined;
    const key = Object.keys(p.gitSync).find((k) => normPath(k) === normPath(path));
    return key ? p.gitSync[key] : undefined;
  };
  const waits = createMemo(() => waiting(roots()));
  // Totals for the folded SESSIONS plate: repos · worktrees (spaces) · sessions (panes with an agent).
  const totals = createMemo(() => {
    const t = p.tree;
    const repos = roots().filter((r) => r.kind === "repo").length;
    const spaces = t?.workspaces.length ?? 0;
    const sessions = t?.panes.filter((x) => x.agent).length ?? 0;
    return { repos, spaces, sessions };
  });

  // Manual open/closed overrides; everything else follows `defaultOpen`.
  const [open, setOpen] = createSignal<Record<string, boolean>>(loadOpen(storage()));
  const isOpen = (n: Node) => open()[n.id] ?? defaultOpen(n, p.selectedPane);
  const toggle = (n: Node, e: MouseEvent) => {
    e.stopPropagation();
    const next = { ...open(), [n.id]: !isOpen(n) };
    setOpen(next);
    saveOpen(storage(), next);
  };
  const select = (n: Node) => { const pane = firstPane(n); if (pane) p.onSelectPane(pane.pane_id); };
  const stop = (e: MouseEvent, fn: () => void) => { e.stopPropagation(); fn(); };

  const Caret = (q: { node: Node; color: string }) => (
    <button class={`car ${q.color}`} title={isOpen(q.node) ? "collapse" : "expand"} onClick={(e) => toggle(q.node, e)}>
      {isOpen(q.node) ? "▾" : "▸"}
    </button>
  );

  const SessionRow = (q: { s: SessionNode; level: "tab" | "pane"; tab: Tab }) => {
    const ctx = () => ctxPct(q.s.agent?.tokens ?? q.s.pane.tokens);
    const shell = !q.s.agent && !q.s.pane.agent;
    return (
      <div class={`row ${q.level} ${q.s.id === p.selectedPane ? "sel" : ""}`}
           title={q.s.pane.cwd ?? undefined}
           onClick={() => p.onSelectPane(q.s.id)}
           onContextMenu={(e) => { e.preventDefault(); q.level === "tab" ? p.onTabMenu(e, q.tab) : p.onPaneMenu(e, q.s.pane); }}>
        <span class="car sp" />
        <span class={`st ${statusClass(q.s.status)}`} />
        <span class={`nm ${shell ? "sh" : ""}`}>{q.level === "tab" ? q.tab.label : q.s.label}</span>
        <span class="kind">{q.s.agentKind ? q.s.agentKind.toUpperCase() : "—"}</span>
        <Show when={ctx() !== null}>
          <span class="seg small"><i style={{ width: `${ctx()}%` }} /></span>
          <span class="pc">{ctx()}</span>
        </Show>
        <span class={`tag ${statusClass(q.s.status)}`}>{statusWord(q.s.status, shell)}</span>
        <button class="add" title="split (pane.split)" onClick={(e) => stop(e, () => p.onSplit(q.s.id))}>+</button>
      </div>
    );
  };

  const TabRow = (q: { t: TabNode }) => (
    <Show when={!q.t.single} fallback={<SessionRow s={q.t.single!} level="tab" tab={q.t.tab} />}>
      <div class={`row tab ${!isOpen(q.t) && q.t.sessions.some((s) => s.id === p.selectedPane) ? "sel" : ""}`}
           onClick={() => select(q.t)} onContextMenu={(e) => { e.preventDefault(); p.onTabMenu(e, q.t.tab); }}>
        <Caret node={q.t} color="cy" />
        <span class={`st ${statusClass(q.t.status)}`} />
        <span class="nm">{q.t.tab.label}</span>
        <button class="add" title="split (pane.split)" onClick={(e) => stop(e, () => { const pane = firstPane(q.t); if (pane) p.onSplit(pane.pane_id); })}>+</button>
        <span class="cnt">{q.t.sessions.length}</span>
      </div>
      <Show when={isOpen(q.t)}>
        <div class="kids t"><For each={q.t.sessions}>{(s) => <SessionRow s={s} level="pane" tab={q.t.tab} />}</For></div>
      </Show>
    </Show>
  );

  const SpaceRow = (q: { s: SpaceNode }) => (
    <>
      <div class={`row space ${!isOpen(q.s) && q.s.tabs.some((t) => t.sessions.some((x) => x.id === p.selectedPane)) ? "sel" : ""}`}
           title={q.s.workspace.worktree?.checkout_path ?? undefined}
           onClick={() => select(q.s)} onContextMenu={(e) => { e.preventDefault(); p.onWorkspaceMenu(e, q.s.workspace); }}>
        <Caret node={q.s} color="red" />
        <span class={`st ${statusClass(q.s.status)}`} />
        <span class="nm">{q.s.workspace.worktree?.branch ?? q.s.workspace.label}</span>
        <Show when={q.s.root}><span class="badge root" title="primary checkout">root</span></Show>
        <Show when={q.s.linked}><span class="badge" title="linked worktree">⎇</span></Show>
        <Show when={gitOf(q.s)}>{(g) => (
          <>
            <Show when={g().behind}><span class="git behind" title={`${g().behind} commit(s) behind ${g().upstream ?? "upstream"} · right-click → PULL`}>↓{g().behind}</span></Show>
            <Show when={g().ahead}><span class="git ahead" title={`${g().ahead} commit(s) ahead of ${g().upstream ?? "upstream"}`}>↑{g().ahead}</span></Show>
          </>
        )}</Show>
        <button class="add" title="new session in this worktree (tab.create)" onClick={(e) => stop(e, () => p.onNewTab(q.s.id))}>+</button>
        <span class="cnt">{q.s.tabs.length}</span>
      </div>
      <Show when={isOpen(q.s)}>
        <div class="kids w"><For each={q.s.tabs}>{(t) => <TabRow t={t} />}</For></div>
      </Show>
    </>
  );

  const RepoRow = (q: { r: RepoNode }) => (
    <>
      <div class="row repo" onClick={(e) => toggle(q.r, e)}>
        <Caret node={q.r} color="yel" />
        <span class="nm">{q.r.label}</span>
        <button class="add" title="new worktree in this repo (worktree.create)"
                onClick={(e) => stop(e, () => { const ws = (q.r.spaces.find((s) => s.root) ?? q.r.spaces[0])?.workspace; const root = ws?.worktree?.checkout_path; if (root) p.onNewWorktree(q.r.label, root, serverOf(ws.workspace_id) || undefined); })}>+</button>
        <span class="cnt">{q.r.spaces.length} · {q.r.agents}</span>
      </div>
      <Show when={isOpen(q.r)}>
        <div class="kids r"><For each={q.r.spaces}>{(s) => <SpaceRow s={s} />}</For></div>
      </Show>
    </>
  );

  return (
    <aside class="side">
      <div class="plate-row">
        <div class="plate click" title="click to fold / unfold" onClick={() => setSessionsFolded(!sessionsFolded())}>Sessions {sessionsFolded() ? "▸" : "▾"}</div>
        <button class="plus" title="open a repository (new space)" onClick={() => p.onNewRepo()}>+</button>
        <Show when={sessionsFolded()}>
          <div class="fold-stats" onClick={() => setSessionsFolded(false)}>
            <span class="y"><b>{totals().repos}</b> repo</span><span class="g"><b>{totals().spaces}</b> worktree</span><span class="d"><b>{totals().sessions}</b> session</span>
            <Show when={waits().length}><span class="o"><b>{waits().length}</b> waiting</span></Show>
          </div>
        </Show>
      </div>
      <Show when={!sessionsFolded()}>

      <Show when={waits().length > 0}>
        <div class="pin">
          <div class="pl"><span>WAITING · {waits().length}</span><span>F2 ↵</span></div>
          <For each={waits()}>
            {(w) => {
              const i = w.path.lastIndexOf(" / ");
              return (
                <div class={`row ${w.session.id === p.selectedPane ? "sel" : ""}`} onClick={() => p.onSelectPane(w.session.id)}
                     onContextMenu={(e) => { e.preventDefault(); p.onPaneMenu(e, w.session.pane); }}>
                  <span class="st b" />
                  <span class="nm"><span class="path">{i >= 0 ? w.path.slice(0, i + 3) : ""}</span>{i >= 0 ? w.path.slice(i + 3) : w.path}</span>
                  <span class="kind">{w.session.agentKind?.toUpperCase() ?? ""}</span>
                </div>
              );
            }}
          </For>
        </div>
      </Show>

      <Show when={!p.empty} fallback={p.empty?.() as never}>
      <Show when={p.tree} fallback={<div class="row micro">connecting…</div>}>
        <Show when={(p.sections?.length ?? 0) > 1} fallback={
          <div class="tree">
            <For each={roots()}>{(r) => (r.kind === "repo" ? <RepoRow r={r} /> : <SpaceRow s={r} />)}</For>
          </div>}>
          <div class="tree">
            <For each={p.sections}>{(sec) => {
              const secRoots = createMemo(() => buildTree(sec.tree));
              const panes = () => sec.tree?.panes.length ?? 0;
              return (
                <div class={`srv-sec ${sec.error ? "down" : ""}`}>
                  <div class="row srv" title={sec.error ?? sec.name}>
                    <span class={`dot ${sec.error ? "bad" : sec.connecting ? "wait" : "ok"}`} aria-hidden="true" />
                    <span class="nm">{sec.name.toUpperCase()}</span>
                    <Show when={sec.error}><span class="tag b">OFFLINE</span></Show>
                    <button class="add" title={`open a repository on ${sec.name}`} onClick={(e) => stop(e, () => p.onNewRepo(sec.id))}>+</button>
                    <span class="cnt">{panes()}</span>
                  </div>
                  <div class="srv-body" inert={!!sec.error} aria-disabled={!!sec.error}>
                    <For each={secRoots()}>{(r) => (r.kind === "repo" ? <RepoRow r={r} /> : <SpaceRow s={r} />)}</For>
                    <Show when={!secRoots().length && !sec.error}><div class="row micro">no spaces · + opens a repository</div></Show>
                  </div>
                </div>
              );
            }}</For>
          </div>
        </Show>
        <div class="lg tree-lg"><span class="y">■ REPO</span><span class="g">■ WORKTREE</span><span class="d">■ SESSION</span><span>□ SHELL</span></div>
      </Show>
      </Show>
      </Show>

      <div class="plate-row" style="margin-top:14px">
        <div class="plate click" title={`Claude accounts of ${p.accountsServer || "this server"} · click to fold / unfold`} onClick={() => setAccountsFolded(!accountsFolded())}>
          Accounts {accountsFolded() ? "▸" : "▾"}
        </div>
        <Show when={p.onAddAccount && p.accounts?.managed !== false}>
          <button class="plus" title="add account: sign in to Claude on this machine" onClick={() => p.onAddAccount?.()}>+</button>
        </Show>
      </div>
      <Show when={p.accountsServer}>{(name) => <div class="micro acc-where" title="the machine of the selected session">ON {name().toUpperCase()}</div>}</Show>
      <Show when={accountsFolded() && p.accounts}>
        {(acc) => {
          const active = () => acc().accounts.find((a) => a.active);
          const u = () => (active() ? usageOf(active()!) : null);
          return (
            <div class="bio fold" onClick={() => setAccountsFolded(false)} title={active()?.email ?? ""}>
              <div class="hd"><span class="acc">{active() ? accountLabel(active()!) : "—"}</span>
                <span class="tag">5H <b>{u()?.five ?? "--"}</b> · 7D <b>{u()?.week ?? "--"}</b></span></div>
            </div>
          );
        }}
      </Show>
      <Show when={!accountsFolded()}>
      <Show when={p.accountsError}>
        {(e) => <div class="row micro" style="color:var(--red)" title={e()}>accounts unavailable: {e()}</div>}
      </Show>
      <Show when={p.accounts?.managed === false}>
        <div class="row micro" title="a stock Herdr server keeps no accounts; deck-server does">accounts are not managed on this server</div>
      </Show>
      <Show when={p.accounts?.managed !== false && p.accounts && !p.accounts.accounts.length}>
        <div class="row micro click" onClick={() => p.onAddAccount?.()}>no account on this machine · + SIGN IN</div>
      </Show>
      <Show when={p.accounts?.managed !== false ? p.accounts : undefined} fallback={<Show when={!p.accountsError && !p.accounts}><div class="row micro">accounts…</div></Show>}>
        {(acc) => (
          <For each={acc().accounts}>
            {(a, i) => {
              const u = usageOf(a);
              const busy = () => p.switching === a.number;
              const failed = () => { const e = p.switchError; return e && e.number === a.number ? e.message : null; };
              const canSwitch = () => !a.active && p.switching == null;
              return (
                <div class={`bio ${a.active ? "" : "off"}`} onDblClick={() => canSwitch() && p.onSwitchAccount(a.number)}
                     onContextMenu={(e) => { e.preventDefault(); p.onAccountMenu(e, a); }}
                     title={a.active ? a.email : `${a.email} — double-click to switch`}>
                  <div class="hd"><span class="num">{String(i() + 1).padStart(2, "0")}</span><span class="acc">{accountLabel(a)}</span>
                    <span class="tag">{a.active ? "ACTIVE" : busy() ? "SWITCHING…" : "DBL-CLICK · SWITCH"}</span></div>
                  <Show when={failed()}>{(m) => <div class="micro" style="color:var(--red)" title={m()}>switch failed: {m()}</div>}</Show>
                  <Quota label="5H" pct={u.five} />
                  <Quota label="7D" pct={u.week} />
                  <Show when={a.active}>
                    <div class="cool">
                      <div><div class="ic">⟳</div><div class="t">{u.fiveReset ?? "--"}</div><div class="l">5H RESET</div></div>
                      <div><div class="ic">⟳</div><div class="t">{u.weekReset ?? "--"}</div><div class="l">7D RESET</div></div>
                    </div>
                  </Show>
                </div>
              );
            }}
          </For>
        )}
      </Show>
      </Show>

      <SessionMap tree={p.tree} repos={roots().filter((r) => r.kind === "repo").length} selectedPane={p.selectedPane} activity={p.activity} now={p.now} onSelectPane={p.onSelectPane} folded={mapFolded()} onFold={setMapFolded} />
    </aside>
  );
}

function Quota(p: { label: string; pct: number | null }) {
  const hot = () => (p.pct ?? 0) >= 80;
  return (
    <div class="qrow">
      <span class="lb">{p.label}</span>
      <span class={`seg ${hot() ? "hot" : ""}`}><i style={{ width: `${p.pct ?? 0}%` }} /></span>
      <span class={`pc ${hot() ? "hot" : ""}`}>{p.pct ?? "--"}</span>
    </div>
  );
}

/** red = working · yellow = waiting for you · teal = stopped recently · dark = idle · outline = shell. */
function cellClass(c: { pane?: Pane; status: AgentStatus; last: number }, now: number): string {
  if (!c.pane?.agent) return "";
  if (c.status === "working") return "w";
  if (c.status === "blocked") return "b";
  return c.last && now - c.last < ACTIVE_MS ? "a" : "i";
}

/** An agent that stopped less than this long ago is "active" (teal), later "idle" (dark). */
const ACTIVE_MS = 15 * 60_000;

function SessionMap(p: { tree?: Tree; repos: number; selectedPane: string | null; activity?: Map<string, number>; now?: number; onSelectPane: (id: string) => void; folded: boolean; onFold: (v: boolean) => void }) {
  const cells = () => {
    const t = p.tree;
    if (!t) return [];
    const act = p.activity ?? new Map<string, number>();
    return t.tabs.map((tab, order) => {
      const panes = t.panes.filter((x) => x.tab_id === tab.tab_id);
      const agent = panes.find((x) => x.agent);
      const ws = t.workspaces.find((w) => w.workspace_id === tab.workspace_id);
      const last = Math.max(0, ...panes.map((x) => act.get(x.pane_id) ?? 0));
      return { tab, order, last, pane: agent ?? panes[0], status: tab.agent_status, title: `${ws?.label ?? ""} / ${tab.label}` };
    // Most recent output first; tabs that have not spoken since Deck started keep Herdr's order after them.
    }).sort((a, b) => b.last - a.last || a.order - b.order);
  };
  // Folded: the same cells grouped by state, as counts.
  const byState = createMemo(() => {
    const now = p.now ?? Date.now();
    const n: Record<string, number> = { w: 0, b: 0, a: 0, i: 0, "": 0 };
    for (const c of cells()) n[cellClass(c, now)] += 1;
    return n;
  });
  return (
    <>
      <div class="plate-row">
        <div class="plate click" style="margin-top:14px" title="click to fold / unfold" onClick={() => p.onFold(!p.folded)}>Session map {p.folded ? "▸" : "▾"}</div>
        <Show when={p.folded}>
          <div class="fold-stats map-stats" style="margin-top:14px" onClick={() => p.onFold(false)}>
            <span class="r"><b>{byState().w}</b> working</span><span class="y"><b>{byState().b}</b> waiting</span><span class="t"><b>{byState().a}</b> active</span><span class="m"><b>{byState().i}</b> idle</span><span><b>{byState()[""]}</b> shell</span>
          </div>
        </Show>
      </div>
      <Show when={!p.folded}>
      <div class="map">
        <div class="hd"><span class="micro">{p.repos} REPOS · {p.tree?.workspaces.length ?? 0} SPACES · {p.tree?.tabs.length ?? 0} TABS · {p.tree?.agents.length ?? 0} AGENTS</span></div>
      <div class="cells">
        <For each={cells()}>
          {(c) => (
            <b class={`${cellClass(c, p.now ?? Date.now())} ${c.pane?.pane_id === p.selectedPane ? "cur" : ""}`}
               title={`${c.title}${c.last ? ` · ${new Date(c.last).toLocaleString([], { day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit" })}` : ""}`} onClick={() => c.pane && p.onSelectPane(c.pane.pane_id)} />
          )}
        </For>
      </div>
      <div class="lg"><span class="r">■ WORKING</span><span class="y">■ WAITING</span><span class="t">■ ACTIVE</span><span class="m">■ IDLE</span><span>□ SHELL</span></div>
      </div>
      </Show>
    </>
  );
}
