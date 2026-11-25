import { For, Show, createResource, createSignal } from "solid-js";
import { api } from "../api";
import GitHubSignInCard from "./GitHubSignInCard";

interface GitHubStatus { available: boolean; signed_in: boolean; user?: string | null }

/**
 * PREFERENCES › CONNECTIONS: what each machine (its deck-server) is signed
 * in to. GitHub for now (the machine's GitHub CLI login, which git there
 * uses); asked over the server's connection like everything else.
 */
export default function Connections(p: { servers: { id: string; name: string }[] }) {
  return (
    <div class="conns">
      <For each={p.servers} fallback={<div class="micro empty-srv">No deck-server connected. Connections live on a deck-server's machine.</div>}>
        {(s) => <Machine id={s.id} name={s.name} />}
      </For>
    </div>
  );
}

function Machine(p: { id: string; name: string }) {
  const [gh, { refetch }] = createResource(() => p.id, (id) => api.machine<GitHubStatus>(id, "deck.github.status").catch((e) => ({ error: String(e).replace(/^Error: /, "") })));
  const [signing, setSigning] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const status = () => { const v = gh(); return v && !("error" in v) ? (v as GitHubStatus) : null; };
  const signOut = () => {
    setBusy(true); setError(null);
    api.machine(p.id, "deck.github.logout").then(() => refetch()).catch((e) => setError(String(e).replace(/^Error: /, ""))).finally(() => setBusy(false));
  };
  return (
    <div class="conn-machine">
      <div class="conn-h"><span class="nm">{p.name}</span></div>
      <div class="conn-row">
        <span class="conn-kind">GITHUB</span>
        <span class="conn-state">
          <Show when={!gh.loading} fallback="asking…">
            <Show when={status()} fallback={<span class="err">{(gh() as { error?: string } | undefined)?.error ?? "unknown"}</span>}>
              {(s) => s().signed_in ? <>signed in as <b>{s().user ?? "?"}</b></> : <span class="micro">not signed in{s().available ? "" : " · the GitHub CLI comes with the first sign-in"}</span>}
            </Show>
          </Show>
        </span>
        <span class="sp" />
        <Show when={status()?.signed_in} fallback={<button class="btn pri" disabled={signing() || gh.loading} onClick={() => setSigning(true)}>SIGN IN</button>}>
          <button class="btn" disabled={busy()} onClick={signOut}>{busy() ? "…" : "SIGN OUT"}</button>
        </Show>
      </div>
      <Show when={error()}>{(e) => <div class="err">{e()}</div>}</Show>
      <Show when={signing()}>
        <GitHubSignInCard server={p.id} where={p.name} onDone={() => { setSigning(false); refetch(); }} onCancel={() => setSigning(false)} />
      </Show>
    </div>
  );
}
