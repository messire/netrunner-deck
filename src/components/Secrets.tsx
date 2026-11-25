import { For, Index, Show, createMemo, createSignal } from "solid-js";
import { api } from "../api";
import { applyRows, parseEnv, rowsOf, validKey, type EnvDoc, type EnvRow } from "../secrets";

interface EnvFile { path: string; exists: boolean; text: string }
interface Row extends EnvRow { shown: boolean }

const errorOf = (e: unknown) => String(e).replace(/^Error: /, "");

/**
 * PREFERENCES › SECRETS: `~/.claude/.env` on each machine (its deck-server),
 * the environment Claude Code reads there, as a KEY / VALUE table. Read and
 * written over the server's connection (`deck.secrets.read` / `.write`);
 * Deck holds the values only while this panel is open and never keeps them.
 */
export default function Secrets(p: { servers: { id: string; name: string }[] }) {
  return (
    <div class="conns">
      <For each={p.servers} fallback={<div class="micro empty-srv">No deck-server connected. Secrets live on a deck-server's machine.</div>}>
        {(s) => <Machine id={s.id} name={s.name} />}
      </For>
    </div>
  );
}

function Machine(p: { id: string; name: string }) {
  const [file, setFile] = createSignal<EnvFile | null>(null);
  const [doc, setDoc] = createSignal<EnvDoc>(parseEnv(""));
  const [rows, setRows] = createSignal<Row[]>([]);
  const [loading, setLoading] = createSignal(false);
  const [saving, setSaving] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  const load = () => {
    setLoading(true); setError(null);
    api.machine<EnvFile>(p.id, "deck.secrets.read")
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
  const changed = () => !!file() && text() !== file()!.text;
  const edit = (i: number, patch: Partial<Row>) => setRows((rs) => rs.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  const add = () => setRows((rs) => [...rs, { line: null, key: "", value: "", shown: true }]);
  const remove = (i: number) => setRows((rs) => rs.filter((_, j) => j !== i));
  const save = () => {
    setSaving(true); setError(null);
    api.machine(p.id, "deck.secrets.write", { text: text() })
      .then(load)
      .catch((e) => setError(errorOf(e)))
      .finally(() => setSaving(false));
  };

  return (
    <div class="conn-machine">
      <div class="conn-h sec-h">
        <span class="nm">{p.name}</span>
        <span class="micro sec-path" title={file()?.path}>{file()?.path ?? "~/.claude/.env"}</span>
        <span class="sp" />
        <button class="btn" disabled={loading() || saving()} onClick={load} title={changed() ? "discard the changes and read the file again" : "read the file again"}>RELOAD</button>
        <button class="btn pri" disabled={!changed() || bad() || saving() || loading()} onClick={save}>{saving() ? "SAVING…" : "SAVE"}</button>
      </div>
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
              <Show when={changed() && !bad()}><span class="micro">unsaved changes</span></Show>
            </div>
          </>
        )}
      </Show>
      <Show when={error()}>{(e) => <div class="err" role="alert">{e()}</div>}</Show>
    </div>
  );
}
