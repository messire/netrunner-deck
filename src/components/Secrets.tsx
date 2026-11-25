import { For, Index, Show, createEffect, createMemo, createSignal, onCleanup } from "solid-js";
import { api } from "../api";
import { applyRows, mergeEnv, parseEnv, rowsOf, validKey, type EnvDoc, type EnvRow } from "../secrets";
import type { MachineView } from "../servers";

interface EnvFile { path: string; exists: boolean; text: string }
interface Row extends EnvRow { shown: boolean }
/** COPY TO waiting for its confirmation. */
interface Pending { to: MachineView; added: number; replaced: number }

const errorOf = (e: unknown) => String(e).replace(/^Error: /, "");
const read = (id: string) => api.machine<EnvFile>(id, "deck.secrets.read");

/**
 * PREFERENCES › SECRETS: `~/.claude/.env` on one machine at a time (every
 * machine with a connected server; the selected session's first), the
 * environment Claude Code reads there, as a KEY / VALUE table. Read and
 * written over that server's connection (`deck.secrets.read` / `.write`,
 * answered by Deck's bridge there); COPY TO merges this file into another
 * machine's by key. Deck holds the values only while this panel is open.
 */
export default function Secrets(p: { machines: MachineView[]; active?: string; onDirty: (dirty: boolean) => void }) {
  const [picked, setPicked] = createSignal<string | null>(null);
  const [dirty, setDirty] = createSignal(false);
  const current = () => p.machines.find((m) => m.key === picked()) ?? p.machines.find((m) => m.key === p.active) ?? p.machines[0];
  createEffect(() => p.onDirty(dirty()));
  onCleanup(() => p.onDirty(false));
  return (
    <div class="secrets">
      <Show when={p.machines.length} fallback={<div class="micro empty-srv">No server connected. Secrets live on a server's machine.</div>}>
        <div class="sec-tabs" role="tablist" aria-label="machine">
          <For each={p.machines}>{(m) => (
            <button class={`btn${current()?.key === m.key ? " on" : ""}`} role="tab" aria-selected={current()?.key === m.key}
                    disabled={dirty() && current()?.key !== m.key} title={dirty() && current()?.key !== m.key ? "SAVE or RELOAD first" : m.name}
                    onClick={() => setPicked(m.key)}>{m.name}</button>
          )}</For>
        </div>
        <Show when={current()} keyed>
          {(m) => <Machine machine={m} others={p.machines.filter((x) => x.key !== m.key)} onDirty={setDirty} />}
        </Show>
      </Show>
    </div>
  );
}

function Machine(p: { machine: MachineView; others: MachineView[]; onDirty: (dirty: boolean) => void }) {
  const [file, setFile] = createSignal<EnvFile | null>(null);
  const [doc, setDoc] = createSignal<EnvDoc>(parseEnv(""));
  const [rows, setRows] = createSignal<Row[]>([]);
  const [loading, setLoading] = createSignal(false);
  const [saving, setSaving] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [copyMenu, setCopyMenu] = createSignal(false);
  const [pending, setPending] = createSignal<Pending | null>(null);
  const [copied, setCopied] = createSignal<string | null>(null);

  const load = () => {
    setLoading(true); setError(null); setPending(null);
    read(p.machine.id)
      .then((f) => {
        const d = parseEnv(f.text);
        setFile(f); setDoc(d); setRows(rowsOf(d).map((r) => ({ ...r, shown: false })));
      })
      .catch((e) => setError(errorOf(e)))
      .finally(() => setLoading(false));
  };
  load();

  const text = createMemo(() => applyRows(doc(), rows()));
  const bad = createMemo(() => rows().some((r) => !validKey(r.key) && (r.key !== "" || r.value !== "")));
  const changed = createMemo(() => !!file() && text() !== file()!.text);
  createEffect(() => p.onDirty(changed()));
  const edit = (i: number, patch: Partial<Row>) => setRows((rs) => rs.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  const add = () => setRows((rs) => [...rs, { line: null, key: "", value: "", shown: true }]);
  const remove = (i: number) => setRows((rs) => rs.filter((_, j) => j !== i));
  const save = () => {
    setSaving(true); setError(null); setCopied(null);
    api.machine(p.machine.id, "deck.secrets.write", { text: text() })
      .then(load)
      .catch((e) => setError(errorOf(e)))
      .finally(() => setSaving(false));
  };

  /** COPY TO: read the target's file and say what the merge would do. */
  const copyTo = (to: MachineView) => {
    setCopyMenu(false); setError(null); setCopied(null); setSaving(true);
    read(to.id)
      .then((t) => { const m = mergeEnv(t.text, file()!.text); setPending({ to, added: m.added, replaced: m.replaced }); })
      .catch((e) => setError(`${to.name}: ${errorOf(e)}`))
      .finally(() => setSaving(false));
  };
  /** The merge again on the target's file as it is now, then written there. */
  const confirmCopy = () => {
    const c = pending();
    if (!c) return;
    setSaving(true); setError(null);
    read(c.to.id)
      .then((t) => api.machine(c.to.id, "deck.secrets.write", { text: mergeEnv(t.text, file()!.text).text }))
      .then(() => { setPending(null); setCopied(`copied to ${c.to.name}`); })
      .catch((e) => setError(`${c.to.name}: ${errorOf(e)}`))
      .finally(() => setSaving(false));
  };
  const busy = () => loading() || saving();

  return (
    <div class="conn-machine sec-machine">
      <div class="conn-h sec-h">
        <span class="micro sec-path" title={file()?.path}>{file()?.path ?? "~/.claude/.env"}</span>
        <span class="sp" />
        <div class="sec-copy">
          <button class="btn" disabled={busy() || !file()?.exists || changed() || !p.others.length || !!pending()} aria-haspopup="menu" aria-expanded={copyMenu()}
                  title={changed() ? "SAVE or RELOAD first" : !p.others.length ? "no other machine connected" : "merge this file into another machine's .env"}
                  onClick={() => setCopyMenu(!copyMenu())}>COPY TO ▾</button>
          <Show when={copyMenu()}>
            <div class="sec-menu" role="menu" onMouseLeave={() => setCopyMenu(false)}>
              <For each={p.others}>{(m) => <button role="menuitem" onClick={() => copyTo(m)}>{m.name}</button>}</For>
            </div>
          </Show>
        </div>
        <button class="btn" disabled={busy()} onClick={load} title={changed() ? "discard the changes and read the file again" : "read the file again"}>RELOAD</button>
        <button class="btn pri" disabled={!changed() || bad() || busy()} onClick={save}>{saving() ? "…" : "SAVE"}</button>
      </div>
      <Show when={pending()}>
        {(c) => (
          <div class="sec-confirm" role="alert">
            <span>{c().added} new, {c().replaced} replaced on <b>{c().to.name}</b></span>
            <span class="sp" />
            <button class="btn pri" disabled={saving() || (c().added === 0 && c().replaced === 0)} onClick={confirmCopy}>COPY</button>
            <button class="btn" disabled={saving()} onClick={() => setPending(null)}>CANCEL</button>
          </div>
        )}
      </Show>
      <Show when={file()} fallback={<div class="micro">{loading() ? "reading…" : ""}</div>}>
        {(f) => (
          <>
            <Show when={!f().exists}>
              <div class="micro">No .env on this machine yet. SAVE creates it.</div>
            </Show>
            <table class="sec-table">
              <thead><tr><th>KEY</th><th>VALUE</th><th /></tr></thead>
              <tbody>
                <Index each={rows()} fallback={<tr><td colspan="3" class="micro">no entries</td></tr>}>
                  {(r, i) => (
                    <tr>
                      <td>
                        <input class={`sec-key${r().key && !validKey(r().key) ? " invalid" : ""}`} value={r().key} placeholder="KEY"
                               aria-label="key" spellcheck={false} autocomplete="off"
                               onInput={(e) => edit(i, { key: e.currentTarget.value.trim() })} />
                      </td>
                      <td class="sec-val">
                        <input type={r().shown ? "text" : "password"} value={r().value} placeholder="value" aria-label={`value of ${r().key || "new key"}`}
                               spellcheck={false} autocomplete="off" onInput={(e) => edit(i, { value: e.currentTarget.value })} />
                        <button class="btn sec-eye" aria-pressed={r().shown} title={r().shown ? "hide the value" : "show the value"}
                                aria-label={r().shown ? "hide value" : "show value"} onClick={() => edit(i, { shown: !r().shown })}>{r().shown ? "◉" : "◎"}</button>
                      </td>
                      <td class="act"><button class="btn" title="delete this entry" aria-label={`delete ${r().key || "entry"}`} onClick={() => remove(i)}>✕</button></td>
                    </tr>
                  )}
                </Index>
              </tbody>
            </table>
            <div class="add">
              <button class="btn" onClick={add}>+ ADD</button>
              <Show when={bad()}><span class="err">a key is letters, digits and _, not starting with a digit</span></Show>
              <Show when={changed() && !bad()}><span class="micro sec-unsaved">unsaved changes · SAVE or RELOAD</span></Show>
              <Show when={!changed() && copied()}>{(c) => <span class="micro">{c()}</span>}</Show>
            </div>
          </>
        )}
      </Show>
      <Show when={error()}>{(e) => <div class="err" role="alert">{e()}</div>}</Show>
    </div>
  );
}
