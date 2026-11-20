import { onCleanup, onMount } from "solid-js";

/** "+" on a repository row: a new linked worktree (branch, optional base ref) inside that repository. */
export default function NewWorktreeBox(p: { repo: string; onCreate: (branch: string, base: string) => void; onClose: () => void }) {
  let branch!: HTMLInputElement;
  let base!: HTMLInputElement;
  const go = () => { const b = branch.value.trim(); if (!b) { branch.focus(); return; } p.onCreate(b, base.value.trim()); p.onClose(); };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") { e.preventDefault(); p.onClose(); }
    else if (e.key === "Enter") { e.preventDefault(); go(); }
    e.stopPropagation();
  };
  onMount(() => { branch.focus(); window.addEventListener("keydown", onKey, true); });
  onCleanup(() => window.removeEventListener("keydown", onKey, true));
  return (
    <div class="veil" onClick={p.onClose}>
      <div class="dlgbox wiz" onClick={(e) => e.stopPropagation()}>
        <div class="dlgt">▞ NEW WORKTREE <span class="crumb"><span>› {p.repo}</span></span></div>
        <label class="wiz-field"><span>branch</span><input ref={branch} class="arc-q wide" placeholder="feature/my-change — existing branch is checked out, a new one is created" /></label>
        <label class="wiz-field"><span>base</span><input ref={base} class="arc-q wide" placeholder="optional: ref to branch from (default HEAD), e.g. origin/main" /></label>
        <div class="dlga"><span class="micro">git worktree add · opens as a space in this repo</span><span class="sp" /><button class="abtn" onClick={p.onClose}>CANCEL</button><button class="abtn ok" onClick={go}>CREATE</button></div>
      </div>
    </div>
  );
}
