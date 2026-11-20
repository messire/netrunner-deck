import { onMount } from "solid-js";

/** One-line text prompt in Deck's own dialog style (centered veil; Enter OK, Esc cancel). */
export default function PromptBox(q: { title: string; value: string; placeholder?: string; okLabel?: string; ok: (v: string) => void; onClose: () => void }) {
  let input!: HTMLInputElement;
  onMount(() => { input.focus(); input.select(); });
  // Read the callbacks before closing: once the owner's signal is null the accessor behind these props is gone.
  const submit = () => { const v = input.value.trim(); const ok = q.ok; q.onClose(); if (v) ok(v); };
  return (
    <div class="veil" onClick={q.onClose}>
      <div class="dlgbox" onClick={(e) => e.stopPropagation()} onKeyDown={(e) => { if (e.key === "Enter") submit(); if (e.key === "Escape") q.onClose(); e.stopPropagation(); }}>
        <div class="dlgt">▞ {q.title}</div>
        <input ref={input} class="arc-q wide" value={q.value} placeholder={q.placeholder ?? ""} />
        <div class="dlga">
          <button class="abtn" onClick={q.onClose}>CANCEL</button>
          <button class="abtn ok" onClick={submit}>{q.okLabel ?? "OK"}</button>
        </div>
      </div>
    </div>
  );
}
