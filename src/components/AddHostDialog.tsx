import { For, Show, createSignal, onMount } from "solid-js";
import { machineErrorOf, validateHostForm, type HostForm, type Probed } from "../servers";

/**
 * SERVERS → + ADD: a machine by user, host and port. CHECK lists its servers
 * over SSH and pings each through that machine's bridge; only the ones that
 * answer are offered, each with a checkbox (the preferred one pre-checked).
 * ADD saves the machine and connects the checked ones. Esc closes.
 */
export default function AddHostDialog(p: {
  onProbe: (user: string, host: string, port: number) => Promise<Probed[]>;
  onAdd: (user: string, host: string, port: number, sockets: string[]) => Promise<void>;
  onClose: () => void;
}) {
  const [form, setForm] = createSignal<HostForm>({ user: "", host: "", port: "22" });
  const [bad, setBad] = createSignal<{ field: keyof HostForm; error: string } | null>(null);
  const [busy, setBusy] = createSignal<"check" | "add" | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [found, setFound] = createSignal<Probed[] | null>(null);
  const [checked, setChecked] = createSignal<Set<string>>(new Set());
  /** The fields the shown result belongs to (editing them drops it). */
  let checkedFor = "";
  let user!: HTMLInputElement;
  onMount(() => setTimeout(() => user.focus(), 0));

  const set = (k: keyof HostForm, v: string) => {
    setForm({ ...form(), [k]: v });
    if (bad()?.field === k) setBad(null);
    if (found() && JSON.stringify(form()) !== checkedFor) { setFound(null); setError(null); }
  };
  const valid = () => {
    const v = validateHostForm(form());
    if (!v.ok) { setBad({ field: v.field, error: v.error }); return null; }
    return v;
  };
  const check = async () => {
    const v = valid();
    if (!v || busy()) return;
    setBusy("check"); setError(null); setFound(null);
    try {
      const list = await p.onProbe(v.user, v.host, v.port);
      checkedFor = JSON.stringify(form());
      setFound(list);
      const pre = list.filter((s) => s.preferred && !s.connected).map((s) => s.socket);
      setChecked(new Set(pre.length ? pre : list.filter((s) => !s.connected).slice(0, 1).map((s) => s.socket)));
    } catch (e) {
      setError(String(e).replace(/^Error: /, ""));
    } finally {
      setBusy(null);
    }
  };
  const add = async () => {
    const v = valid();
    if (!v || busy() || !checked().size) return;
    setBusy("add"); setError(null);
    try {
      await p.onAdd(v.user, v.host, v.port, [...checked()]);
      p.onClose();
    } catch (e) {
      setError(String(e).replace(/^Error: /, ""));
    } finally {
      setBusy(null);
    }
  };
  const toggle = (socket: string) => {
    const s = new Set(checked());
    if (s.has(socket)) s.delete(socket); else s.add(socket);
    setChecked(s);
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); p.onClose(); }
    else if (e.key === "Enter" && (e.target as HTMLElement).tagName === "INPUT" && (e.target as HTMLInputElement).type !== "checkbox") {
      e.preventDefault(); e.stopPropagation();
      if (found()?.length && JSON.stringify(form()) === checkedFor) add(); else check();
    }
  };
  const field = (k: keyof HostForm, label: string, extra: Record<string, unknown> = {}) => (
    <label class={`hf ${k}${bad()?.field === k ? " bad" : ""}`}>
      <span class="micro">{label}</span>
      <input value={form()[k]} onInput={(e) => set(k, e.currentTarget.value)} spellcheck={false} autocomplete="off"
             aria-label={label} aria-invalid={bad()?.field === k} disabled={!!busy()} {...extra} />
    </label>
  );

  return (
    <div class="veil addhost-veil" onMouseDown={(e) => { if (e.target === e.currentTarget) p.onClose(); }}>
      <div class="dlgbox addhost" role="dialog" aria-modal="true" aria-label="Add a machine" onKeyDown={onKey}>
        <div class="dlgt">▞ ADD MACHINE <span class="crumb">› over your SSH setup, through deck-server bridge there</span></div>
        <div class="hrow">
          {field("user", "USER", { ref: (el: HTMLInputElement) => (user = el), placeholder: "dev" })}
          <span class="at">@</span>
          {field("host", "HOST / ADDRESS", { placeholder: "build-box.example.com" })}
          <span class="at">:</span>
          {field("port", "PORT", { inputmode: "numeric", placeholder: "22" })}
        </div>
        <Show when={bad()}>{(b) => <div class="herr" role="alert">{b().error}</div>}</Show>
        <Show when={busy() === "check"}>
          <div class="hbusy micro">Signing in over SSH, listing servers, pinging each through the bridge…</div>
        </Show>
        <Show when={error()}>{(e) => <div class="herr" role="alert" title={e()}>{machineErrorOf(e())}<span class="raw">{e()}</span></div>}</Show>
        <Show when={found()}>{(list) => (
          <div class="hfound">
            <div class="micro">{list().length} SERVER{list().length === 1 ? "" : "S"} ANSWERED</div>
            <For each={list()}>{(s) => (
              <label class={`hsrv${s.connected ? " on" : ""}`}>
                <input type="checkbox" checked={s.connected || checked().has(s.socket)} disabled={s.connected || !!busy()} onChange={() => toggle(s.socket)} aria-label={`connect ${s.name}`} />
                <span class="nm">{s.name}<span class="addr">{s.socket}</span></span>
                <span class="kind">{s.kind}</span>
                <span class="ver">{s.version ? `${s.version}${s.protocol != null ? ` · p${s.protocol}` : ""}` : "—"}</span>
                <Show when={s.connected}><span class="kind">connected</span></Show>
              </label>
            )}</For>
          </div>
        )}</Show>
        <div class="dlga">
          <span class="micro">Enter · Esc</span><span class="sp" />
          <button class="abtn" onClick={p.onClose}>CANCEL</button>
          <button class="abtn" disabled={!!busy()} onClick={check}>{busy() === "check" ? "CHECKING…" : "CHECK"}</button>
          <button class="abtn ok" disabled={!!busy() || !found()?.length || !checked().size} onClick={add}>{busy() === "add" ? "ADDING…" : "ADD & CONNECT"}</button>
        </div>
      </div>
    </div>
  );
}
