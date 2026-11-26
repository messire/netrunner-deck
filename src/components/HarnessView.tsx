import { For, Show, createMemo, createResource, createSignal, onCleanup, onMount } from "solid-js";
import { marked } from "marked";
import DOMPurify from "dompurify";
import { api, type HarnessItem, type HarnessRepo, type RepoGroup } from "../api";
import { byKind, matchHarness, sectionHarness, tagCounts, untagged } from "../harness";
import { moveRepo } from "../archive-tree";
import type { Source } from "../sources";
import ServerSections, { SourceTag } from "./ServerSections";

/**
 * HARNESS: Claude Code's configuration per repository — what the agent is
 * told (CLAUDE.md, rules), how it is set up (settings, MCP) and what it can
 * do (skills, agents, commands). Personal (`~/.claude`, applies everywhere)
 * is pinned first; the repositories are the archive's, in its groups.
 * It is the picked server's machine that is read (one section per server
 * under ALL); the viewer on the right shows one file at a time.
 */
interface Props {
  sources: Source<HarnessRepo[]>[];
  groups: RepoGroup[];
  onGroups: (groups: RepoGroup[]) => void;
  onRefresh: () => void;
  /** Right-click on PERSONAL or one of its items of `server` (COPY TO / MOVE TO another server). */
  onPersonalMenu?: (server: string, e: MouseEvent, item?: HarnessItem) => void;
}

const md = (text: string): string => DOMPurify.sanitize(marked.parse(text, { async: false }) as string, { USE_PROFILES: { html: true } });
const kb = (n: number) => (n < 1024 ? `${n} B` : `${(n / 1024).toFixed(n < 10240 ? 1 : 0)} KB`);
const when = (ms: number) => (ms ? new Date(ms).toLocaleString([], { day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit" }) : "");

export default function HarnessView(p: Props) {
  const [query, setQuery] = createSignal("");
  // Two servers have a PERSONAL each (and may share repositories): under ALL a fold is per server.
  const keyOf = (server: string, key: string) => (p.sources.length > 1 ? `${server}:${key}` : key);
  // Folds the user changed; PERSONAL starts open, repositories closed.
  const [open, setOpen] = createSignal<Record<string, boolean>>({});
  const opened = (key: string, personal: boolean) => open()[key] ?? personal;
  const toggle = (key: string, personal: boolean) => setOpen((s) => ({ ...s, [key]: !opened(key, personal) }));
  const [current, setCurrentRaw] = createSignal<{ server: string; item: HarnessItem } | null>(null);
  const setCurrent = (server: string, item: HarnessItem) => setCurrentRaw({ server, item });
  const item = () => current()?.item ?? null;
  const [ask, setAsk] = createSignal<{ title: string; value: string; ok: (v: string) => void } | null>(null);

  // Selected tag chips (any-of); the clouds stay complete so a tag can be deselected.
  const [tags, setTags] = createSignal<Set<string>>(new Set());
  const toggleTag = (t: string) => setTags((s) => { const n = new Set(s); if (n.has(t)) n.delete(t); else n.add(t); return n; });
  const all = () => p.sources.flatMap((s) => s.data ?? []);
  const shownCount = createMemo(() => p.sources.reduce((n, s) => n + matchHarness(s.data ?? [], query(), tags()).length, 0));
  // One tag cloud for the whole harness: counts across personal + every repository of every shown server.
  const allTags = createMemo(() => tagCounts(all().flatMap((r) => r.items)));
  const [text] = createResource(() => current() ?? null, (c) => api.harnessRead(c.server, c.item.path));
  const isMd = () => /\.md$/i.test(item()?.path ?? "");

  // CLASSIFY in flight per repo key; `deck://harness` ends it and triggers a rescan.
  const [classifying, setClassifying] = createSignal<Set<string>>(new Set());
  const [notice, setNotice] = createSignal<string | null>(null);
  onMount(() => {
    let un: (() => void) | undefined;
    api.onHarness((e) => {
      setClassifying((s) => { const n = new Set(s); n.delete(e.key); return n; });
      setNotice(e.error ? `✕ ${e.key}: ${e.error}` : `${e.key}: ${e.written ?? 0} file${e.written === 1 ? "" : "s"} tagged`);
      p.onRefresh();
    }).then((u) => (un = u));
    onCleanup(() => un?.());
  });
  const classify = (server: string, r: HarnessRepo) => {
    setNotice(null);
    api.harnessClassify(server, r.key, r.items).then((started) => { if (started) setClassifying((s) => new Set(s).add(r.key)); }, (e) => setNotice(`✕ ${String(e)}`));
  };

  const newGroup = () => setAsk({ title: "NEW GROUP", value: "", ok: (name) => p.onGroups([...p.groups, { id: `g${Date.now().toString(36)}`, name, repos: [], hidden: false }]) });

  const PromptBox = (q: { title: string; value: string; ok: (v: string) => void }) => {
    let input!: HTMLInputElement;
    onMount(() => { input.focus(); input.select(); });
    const submit = () => { const v = input.value.trim(); const ok = q.ok; setAsk(null); if (v) ok(v); };
    return (
      <div class="veil" onClick={() => setAsk(null)}>
        <div class="dlgbox" onClick={(e) => e.stopPropagation()} onKeyDown={(e) => { if (e.key === "Enter") submit(); if (e.key === "Escape") setAsk(null); }}>
          <div class="dlgt">▞ {q.title}</div>
          <input ref={input} class="arc-q wide" value={q.value} placeholder="group name" />
          <div class="dlga"><button class="abtn" onClick={() => setAsk(null)}>CANCEL</button><button class="abtn ok" onClick={submit}>OK</button></div>
        </div>
      </div>
    );
  };

  const Repo = (q: { r: HarnessRepo; group: RepoGroup | null; src: Source<HarnessRepo[]> }) => {
    const k = () => keyOf(q.src.id, q.r.key);
    const isOpen = () => opened(k(), q.r.personal) || !!query() || tags().size > 0;
    const total = () => q.r.items.length;
    // The full repo (before filtering) feeds the tag cloud and CLASSIFY.
    const full = () => q.src.data?.find((x) => x.key === q.r.key) ?? q.r;
    return (
      <>
        <div class={`arow repo hrepo ${isOpen() ? "open" : ""} ${q.r.personal ? "personal" : ""}`} onClick={() => toggle(k(), q.r.personal)} title={q.r.root}
          onContextMenu={(e) => { if (q.r.personal && p.onPersonalMenu) { e.preventDefault(); p.onPersonalMenu(q.src.id, e); } }}>
          <span class="gem" classList={{ root: q.r.personal }} />
          <span class={`nm ${q.r.personal ? "pn" : "rn"}`}>{q.r.name}</span>
          <span class="path mono">{q.r.root}</span>
          <span class="mono cnt" title="files">{q.r.exists ? total() : "—"}</span>
          <Show when={!q.r.exists}><span class="micro" style="color:var(--org)">CHECKOUT MISSING</span></Show>
          <span class="sp" />
          <Show when={!q.src.remote && untagged(full().items)}>{(n) => (
            <button class="abtn" disabled={classifying().has(q.r.key)} onClick={(e) => { e.stopPropagation(); classify(q.src.id, full()); }}
              title={`shadow claude -p (haiku) tags the ${n()} untagged skills/agents/commands/rules and writes tags: into their frontmatter`}>
              {classifying().has(q.r.key) ? "CLASSIFYING…" : `CLASSIFY ${n()}`}
            </button>
          )}</Show>
          <Show when={!q.r.personal}>
            <select class="asel" title="move to a group" value={q.group?.id ?? ""} onClick={(e) => e.stopPropagation()} onChange={(e) => p.onGroups(moveRepo(p.groups, q.r.key, e.currentTarget.value || null))}>
              <option value="">MAIN</option>
              <For each={p.groups}>{(g) => <option value={g.id}>{g.name.toUpperCase()}</option>}</For>
            </select>
          </Show>
          <span class="tw">{isOpen() ? "▾" : "▸"}</span>
        </div>
        <Show when={isOpen()}>
          <Show when={total()} fallback={<div class="hnone">no Claude Code configuration here</div>}>
            <For each={byKind(q.r.items)}>{(g) => (
              <div class="hkind">
                <div class="hkl">{g.label} <span class="mono">{g.items.length}</span></div>
                <For each={g.items}>{(it) => (
                  <div class={`arow sess hitem ${current()?.server === q.src.id && item()?.path === it.path ? "sel" : ""}`} onClick={() => setCurrent(q.src.id, it)} title={it.description ?? it.path}
                    onContextMenu={(e) => { if (q.r.personal && p.onPersonalMenu) { e.preventDefault(); p.onPersonalMenu(q.src.id, e, it); } }}>
                    <span class="nm">{it.name}</span>
                    <Show when={it.local}><span class="src">LOCAL</span></Show>
                    <For each={it.tags}>{(t) => <button class={`chip ${tags().has(t) ? "on" : ""}`} onClick={(e) => { e.stopPropagation(); toggleTag(t); }}>#{t}</button>}</For>
                    <Show when={it.description}>{(d) => <span class="hdesc">{d()}</span>}</Show>
                    <span class="sp" />
                    <span class="mono when">{when(it.mtime_ms)}</span>
                    <span class="mono cnt">{kb(it.size)}</span>
                  </div>
                )}</For>
              </div>
            )}</For>
          </Show>
        </Show>
      </>
    );
  };

  return (
    <section class="archive harness">
      <Show when={ask()}>{(q) => <PromptBox title={q().title} value={q().value} ok={q().ok} />}</Show>
      <div class="arc-head">
        <span class="plate">Harness · agent config</span>
        <SourceTag sources={p.sources} />
        <span class="micro">CLAUDE.md · settings · skills · agents · commands · rules · MCP, per repository</span>
        <input class="arc-q" placeholder="search: repo, name, kind, description…" value={query()} onInput={(e) => setQuery(e.currentTarget.value)} />
        <span class="sp" />
        <Show when={p.sources.length === 1 && p.sources[0].error}>{(m) => <span class="micro toast">✕ {m()}</span>}</Show>
        <Show when={notice()}>{(m) => <span class="micro" style={m().startsWith("✕") ? "color:var(--red)" : "color:var(--grn)"}>{m()}</span>}</Show>
        <button class="abtn" onClick={newGroup} title="a group of repositories (shared with ARCHIVE)">+ GROUP</button>
        <button class="abtn" onClick={p.onRefresh}>↻ RESCAN</button>
      </div>
      <Show when={allTags().length}>
        <div class="htags global">
          <For each={allTags()}>{(t) => <button class={`chip ${tags().has(t.tag) ? "on" : ""}`} onClick={() => toggleTag(t.tag)} title={tags().has(t.tag) ? "click to deselect" : `show only #${t.tag}`}>#{t.tag} <i>{t.n}</i></button>}</For>
          <Show when={tags().size}><button class="chip clear" onClick={() => setTags(new Set())} title="deselect all">✕ clear</button></Show>
        </div>
      </Show>
      <div class="hsplit">
        <div class="hlist">
          <ServerSections view="harness" sources={p.sources} count={(s) => (s.data ?? []).reduce((n, r) => n + r.items.length, 0)}>{(src) => {
            const layout = createMemo(() => sectionHarness(matchHarness(src.data ?? [], query(), tags()), p.groups));
            return (
              <Show when={src.data} fallback={<div class="arc-empty">{src.error ? `✕ ${src.error}` : "▞ SCANNING…"}</div>}>
                <Show when={layout().personal}>{(r) => <Repo r={r()} group={null} src={src} />}</Show>
                <For each={layout().sections}>{(sec) => (
                  <Show when={sec.repos.length || sec.group}>
                    <Show when={sec.group} fallback={<div class="hsec">MAIN <span class="mono">{sec.repos.length} repos</span></div>}>
                      {(g) => <div class="hsec" classList={{ hid: g().hidden }}>{g().name.toUpperCase()} <span class="mono">{sec.repos.length} repos</span></div>}
                    </Show>
                    <Show when={!sec.group?.hidden}><For each={sec.repos}>{(r) => <Repo r={r} group={sec.group} src={src} />}</For></Show>
                  </Show>
                )}</For>
              </Show>
            );
          }}</ServerSections>
          <Show when={p.sources.some((s) => s.data) && !shownCount()}><div class="arc-empty">▞ NOTHING MATCHES</div></Show>
        </div>
        <div class="hview">
          <Show when={item()} fallback={<div class="arc-empty">▞ PICK A FILE</div>}>
            {(it) => (
              <>
                <div class="hvh"><span class="nm">{it().name}</span><For each={it().tags}>{(t) => <span class="chip">#{t}</span>}</For><span class="mono path">{it().path}</span><span class="sp" /><span class="mono">{kb(it().size)} · {when(it().mtime_ms)}</span></div>
                <Show when={!text.loading} fallback={<div class="chat-note">loading…</div>}>
                  <Show when={!text.error} fallback={<div class="chat-note err">{String(text.error)}</div>}>
                    <Show when={isMd()} fallback={<pre class="hraw">{text()}</pre>}>
                      <div class="mdtext hmd" innerHTML={md(text() ?? "")} />
                    </Show>
                  </Show>
                </Show>
              </>
            )}
          </Show>
        </div>
      </div>
    </section>
  );
}
