import { For, Show, createEffect, createMemo, createResource, createSignal, on, onCleanup, onMount } from "solid-js";
import { api, type Hack, type HarnessRepo, type WorktreeInfo } from "../api";
import { filterItems, hackFrom, hackItemIn, hackPrompt, hackable, launchLabel, slashName, worktreeChoices } from "../quickhacks";
import Think from "./Think";
import type { Source } from "../sources";
import ServerSections from "./ServerSections";

/**
 * QUICKHACKS — harness entries (skills, commands) pinned as buttons on the
 * right. ADD walks source › repository › entry; a button walks repository ›
 * worktree › agent › arguments and launches a fresh session that starts with
 * `/name args` (the Rust side drives Herdr and the agent's prompt).
 * A hack belongs to the server whose harness it was pinned from and runs
 * there; the panel shows the picked server's (one section each under ALL).
 */
/** One server's part of the panel: its hacks and its harness (for ADD and RUN). */
export type HackSource = Source<{ hacks: Hack[]; harness: HarnessRepo[] | undefined }>;
interface Props {
  /** Every pinned hack (saved as one list). */
  hacks: Hack[];
  sources: HackSource[];
  collapsed: boolean;
  onSave: (hacks: Hack[]) => void;
  onCollapse: (collapsed: boolean) => void;
  onOpenPane: (paneId: string) => void;
  /** Right-click on a hack: COPY TO / MOVE TO another server. */
  onHackMenu?: (e: MouseEvent, hack: Hack) => void;
  onError: (message: string) => void;
}

type Step<T> = { title: string; crumb: string[]; items: T[]; render: (t: T) => { label: string; hint?: string; tag?: string }; pick: (t: T) => void; search?: boolean };

export default function QuickHacks(p: Props) {
  // ---- wizards: one generic step dialog, driven by small state machines ----
  const [step, setStep] = createSignal<Step<unknown> | null>(null);
  const [args, setArgs] = createSignal<{ hack: Hack; wt: WorktreeInfo; repoRoot: string; server: string; agent: string } | null>(null);
  const [running, setRunning] = createSignal<Map<string, string>>(new Map()); // run id → stage text
  const close = () => { setStep(null); setArgs(null); };

  const repos = (src: HackSource) => (src.data?.harness ?? []).filter((r) => !r.personal && r.exists);
  const personal = (src: HackSource) => src.data?.harness?.find((r) => r.personal);
  const total = () => p.sources.reduce((n, s) => n + (s.data?.hacks.length ?? 0), 0);

  // ADD: (server ›) source › repo › entry
  const addStart = () => {
    if (p.sources.length === 1) { addSource(p.sources[0]); return; }
    setStep({
      title: "ADD HACK", crumb: ["server"], search: false,
      items: p.sources, render: (x) => { const s = x as HackSource; return { label: s.name.toUpperCase(), hint: s.error ?? (s.data?.harness ? "its harness" : "harness not read yet"), tag: s.kind === "deck-server" ? "deck" : "herdr" }; },
      pick: (x) => addSource(x as HackSource),
    } as Step<unknown>);
  };
  const addSource = (src: HackSource) => setStep({
    title: "ADD HACK", crumb: [src.name, "source"], search: false,
    items: [{ id: "personal", label: "PERSONAL", hint: "~/.claude — skills and commands that apply everywhere" }, { id: "project", label: "PROJECT", hint: "a repository's own .claude" }],
    render: (x) => { const s = x as { label: string; hint: string }; return { label: s.label, hint: s.hint }; },
    pick: (x) => { const s = x as { id: string }; if (s.id === "personal") { const r = personal(src); if (r) addEntry(src, r); else p.onError(`no ~/.claude found on ${src.name}`); } else addRepo(src); },
  } as Step<unknown>);
  const addRepo = (src: HackSource) => setStep({
    title: "ADD HACK", crumb: ["project", "repository"], search: true,
    items: repos(src), render: (x) => { const r = x as HarnessRepo; return { label: r.name, hint: r.root, tag: `${hackable(r.items).length}` }; },
    pick: (x) => addEntry(src, x as HarnessRepo),
  } as Step<unknown>);
  const addEntry = (src: HackSource, repo: HarnessRepo) => setStep({
    title: "ADD HACK", crumb: [repo.personal ? "personal" : repo.name, "entry"], search: true,
    items: hackable(repo.items), render: (x) => { const i = x as HarnessRepo["items"][number]; return { label: `/${slashName(i)}`, hint: i.description ?? i.path, tag: i.kind }; },
    pick: (x) => {
      const h = { ...hackFrom(repo, x as HarnessRepo["items"][number]), server: src.id };
      if (!p.hacks.some((o) => o.path === h.path && (o.server ?? null) === h.server)) p.onSave([...p.hacks, h]);
      close();
    },
  } as Step<unknown>);

  // RUN (on the hack's server): repo › worktree › agent › args
  const runStart = (hack: Hack, src: HackSource) => {
    const list = repos(src);
    const own = hack.repo_key ? list.find((r) => r.key === hack.repo_key) : undefined;
    const ordered = own ? [own, ...list.filter((r) => r !== own)] : list;
    setStep({
      title: `RUN /${hack.name}`, crumb: ["repository"], search: true,
      items: ordered, render: (x) => { const r = x as HarnessRepo; return { label: r.name, hint: r.root, tag: r === own ? "home" : undefined }; },
      pick: (x) => runWorktree(hack, x as HarnessRepo, src.id),
    } as Step<unknown>);
  };
  const runWorktree = async (hack: Hack, repo: HarnessRepo, server: string) => {
    setStep({ title: `RUN /${hack.name}`, crumb: [repo.name, "worktree"], items: [], render: () => ({ label: "…" }), pick: () => {}, search: false } as Step<unknown>);
    try {
      const r = await api.quickhackWorktrees(repo.root, server);
      const wts = worktreeChoices(r.worktrees ?? []);
      const root = r.source?.repo_root ?? repo.root;
      setStep({
        title: `RUN /${hack.name}`, crumb: [repo.name, "worktree"], search: true,
        items: wts, render: (x) => { const w = x as WorktreeInfo; return { label: w.branch ?? w.label, hint: w.path, tag: w.is_linked_worktree ? (w.open_workspace_id ? "open" : "worktree") : "main" }; },
        pick: (x) => runAgent(hack, x as WorktreeInfo, root, server),
      } as Step<unknown>);
    } catch (e) { close(); p.onError(String(e)); }
  };
  const runAgent = (hack: Hack, wt: WorktreeInfo, repoRoot: string, server: string) => setStep({
    title: `RUN /${hack.name}`, crumb: [wt.branch ?? wt.label, "agent"], search: false,
    items: [{ id: "claude", label: "CLAUDE", hint: "claude — Claude Code" }],
    render: (x) => { const a = x as { label: string; hint: string }; return { label: a.label, hint: a.hint }; },
    pick: (x) => { setStep(null); setArgs({ hack, wt, repoRoot, server, agent: (x as { id: string }).id }); },
  } as Step<unknown>);
  const launch = async (text: string) => {
    const a = args();
    if (!a) return;
    setArgs(null);
    const run = `r${Date.now().toString(36)}`;
    setRunning((m) => new Map(m).set(run, `/${a.hack.name} · opening ${a.wt.branch ?? a.wt.label}`));
    try {
      const res = await api.quickhackLaunch(run, { cwd: a.wt.path, repo_root: a.repoRoot, label: launchLabel(a.hack, a.wt), agent: a.agent, prompt: hackPrompt(a.hack, text) }, a.server);
      p.onOpenPane(res.pane_id);
    } catch (e) {
      setRunning((m) => { const n = new Map(m); n.delete(run); return n; });
      p.onError(String(e));
    }
  };
  onMount(() => {
    let un: (() => void) | undefined;
    api.onQuickhack((e) => {
      setRunning((m) => {
        const n = new Map(m);
        const name = (n.get(e.run) ?? "").split(" · ")[0];
        if (e.stage === "done") { n.delete(e.run); return n; }
        if (e.stage === "error") { p.onError(`${name}: ${String(e.detail)}`); n.delete(e.run); return n; }
        n.set(e.run, `${name} · ${{ workspace: "workspace up", shell: "starting the agent", agent: "agent ready", prompt: "prompt sent" }[e.stage]}`);
        return n;
      });
    }).then((u) => (un = u));
    onCleanup(() => un?.());
  });

  const remove = (h: Hack) => p.onSave(p.hacks.filter((x) => x.id !== h.id));
  const move = (h: Hack, d: -1 | 1) => {
    const i = p.hacks.indexOf(h); const j = i + d;
    if (j < 0 || j >= p.hacks.length) return;
    const next = [...p.hacks]; [next[i], next[j]] = [next[j], next[i]]; p.onSave(next);
  };

  return (
    <aside class={`hacks ${p.collapsed ? "closed" : ""}`}>
      <div class="hk-head" onDblClick={() => p.onCollapse(!p.collapsed)}>
        <button class="hk-fold" onClick={() => p.onCollapse(!p.collapsed)} title={p.collapsed ? "open QUICKHACKS" : "collapse"}>{p.collapsed ? "◀" : "▶"}</button>
        <Show when={!p.collapsed}><span class="plate">Quickhacks</span><span class="sp" /><button class="abtn" onClick={addStart} title="pin a skill or command">+ ADD</button></Show>
      </div>
      <Show when={!p.collapsed} fallback={<div class="hk-rail" onClick={() => p.onCollapse(false)}><span>Q U I C K H A C K S</span><b>{total()}</b></div>}>
        <div class="hk-list">
          <ServerSections view="quickhacks" sources={p.sources} count={(s) => s.data?.hacks.length ?? 0}>{(src) => (
          <Show when={src.data?.hacks.length} fallback={<div class="hk-empty">▞ NO HACKS PINNED<br />+ ADD picks a skill or command from the harness</div>}>
            <For each={src.data?.hacks ?? []}>{(h) => (
              <div class={`hk ${h.source}`} classList={{ missing: hackItemIn(h, src.data?.harness) === null }}
                title={`${h.description ?? ""}\n${h.path}${hackItemIn(h, src.data?.harness) === null ? "\n— not in this server's harness" : ""}`}
                onContextMenu={(e) => { if (p.onHackMenu) { e.preventDefault(); p.onHackMenu(e, h); } }}>
                <button class="hk-btn" onClick={() => runStart(h, src)}>
                  <span class="hk-n">/{h.name}</span>
                  <span class="hk-s">{h.source === "personal" ? "personal" : h.repo_name}<Show when={h.argument_hint}>{(a) => <i> {a()}</i>}</Show><Show when={hackItemIn(h, src.data?.harness) === null}><b class="hk-miss"> · NOT IN THIS HARNESS</b></Show></span>
                  <Show when={h.description}>{(d) => <span class="hk-d">{d()}</span>}</Show>
                </button>
                <div class="hk-ops"><button onClick={() => move(h, -1)} title="up">▴</button><button onClick={() => move(h, 1)} title="down">▾</button><button class="x" onClick={() => remove(h)} title="unpin">✕</button></div>
              </div>
            )}</For>
          </Show>
          )}</ServerSections>
        </div>
        <Show when={running().size}>
          <div class="hk-runs"><For each={[...running().values()]}>{(t) => <div class="hk-run"><Think size={18} /> {t}</div>}</For></div>
        </Show>
      </Show>

      <Show when={step()}>{(s) => <StepBox step={s()} onClose={close} />}</Show>
      <Show when={args()}>{(a) => <ArgsBox hack={a().hack} where={`${a().wt.branch ?? a().wt.label} · ${a().agent.toUpperCase()}`} onRun={launch} onClose={close} />}</Show>
    </aside>
  );
}

/** One wizard step: a (searchable) list; digits/↑↓/Enter pick, Esc closes. */
function StepBox(p: { step: Step<unknown>; onClose: () => void }) {
  const [q, setQ] = createSignal("");
  const [at, setAt] = createSignal(0);
  let input!: HTMLInputElement;
  const rows = createMemo(() => {
    const items = p.step.items.map((it) => ({ it, r: p.step.render(it) }));
    return filterItems(items.map((x) => ({ ...x, name: x.r.label, description: x.r.hint ?? "" })), q());
  });
  createEffect(on(() => [q(), p.step], () => setAt(0)));
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") { e.preventDefault(); p.onClose(); }
    else if (e.key === "Enter") { e.preventDefault(); const r = rows()[at()]; if (r) p.step.pick(r.it); }
    else if (e.key === "ArrowDown") { e.preventDefault(); setAt((i) => Math.min(rows().length - 1, i + 1)); }
    else if (e.key === "ArrowUp") { e.preventDefault(); setAt((i) => Math.max(0, i - 1)); }
    else if (/^[1-9]$/.test(e.key) && !q() && Number(e.key) <= rows().length) { e.preventDefault(); p.step.pick(rows()[Number(e.key) - 1].it); }
    e.stopPropagation();
  };
  onMount(() => { input?.focus(); window.addEventListener("keydown", onKey, true); });
  onCleanup(() => window.removeEventListener("keydown", onKey, true));
  return (
    <div class="veil" onClick={p.onClose}>
      <div class="dlgbox wiz" onClick={(e) => e.stopPropagation()}>
        <div class="dlgt">▞ {p.step.title} <span class="crumb"><For each={p.step.crumb}>{(c) => <span>› {c}</span>}</For></span></div>
        <input ref={input} class="arc-q wide" placeholder={p.step.search ? "search…" : "digits · ↑↓ · Enter"} value={q()} onInput={(e) => setQ(e.currentTarget.value)} readOnly={!p.step.search} />
        <div class="wiz-list">
          <Show when={rows().length} fallback={<div class="hk-empty">{p.step.items.length ? "▞ NOTHING MATCHES" : "▞ …"}</div>}>
            <For each={rows()}>{(r, i) => (
              <button class={`cm-opt ${i() === at() ? "cur" : ""}`} onMouseMove={() => setAt(i())} onClick={() => p.step.pick(r.it)}>
                <span class="n">{i() < 9 ? i() + 1 : ""}</span>
                <span class="lb">{r.r.label}<Show when={r.r.tag}>{(t) => <em class="tag">{t()}</em>}</Show><Show when={r.r.hint}>{(h) => <span class="hint">{h()}</span>}</Show></span>
              </button>
            )}</For>
          </Show>
        </div>
        <div class="dlga"><span class="sp" /><button class="abtn" onClick={p.onClose}>CANCEL</button></div>
      </div>
    </div>
  );
}

/** Last step: the arguments, with the harness's hint and description. */
function ArgsBox(p: { hack: Hack; where: string; onRun: (args: string) => void; onClose: () => void }) {
  let input!: HTMLTextAreaElement;
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") { e.preventDefault(); p.onClose(); }
    else if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); p.onRun(input.value); }
    e.stopPropagation();
  };
  onMount(() => { input.focus(); window.addEventListener("keydown", onKey, true); });
  onCleanup(() => window.removeEventListener("keydown", onKey, true));
  return (
    <div class="veil" onClick={p.onClose}>
      <div class="dlgbox wiz" onClick={(e) => e.stopPropagation()}>
        <div class="dlgt">▞ RUN /{p.hack.name} <span class="crumb"><span>› {p.where}</span><span>› arguments</span></span></div>
        <Show when={p.hack.description}>{(d) => <div class="wiz-desc">{d()}</div>}</Show>
        <div class="wiz-cmd"><span class="sn">/{p.hack.name}</span><Show when={p.hack.argument_hint}>{(a) => <span class="sa">{a()}</span>}</Show></div>
        <textarea ref={input} class="arc-q wide" rows={3} placeholder={p.hack.argument_hint ?? "arguments (optional) · Enter runs · Shift+Enter newline"} />
        <div class="dlga"><span class="micro">a new {p.where} session starts with this prompt</span><span class="sp" /><button class="abtn" onClick={p.onClose}>CANCEL</button><button class="abtn ok" onClick={() => p.onRun(input.value)}>▶ RUN</button></div>
      </div>
    </div>
  );
}
