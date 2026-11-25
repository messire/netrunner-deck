import { Show, createSignal, onCleanup } from "solid-js";
import { api } from "../api";
import { sleep } from "../jobs";

/**
 * Sign in to GitHub on a server's machine (deck-server's `deck.github.*`):
 * the server asks GitHub for a device code, Deck shows it with the link,
 * the user approves in the browser, the server polls GitHub and keeps the
 * token on its own machine. Deck never sees the token.
 */
export default function GitHubSignInCard(p: {
  server: string | null;
  /** Machine name for the title. */
  where?: string;
  /** Why the card is shown. */
  reason?: string;
  onDone: (user: string | null) => void;
  onCancel?: () => void;
}) {
  const [code, setCode] = createSignal<{ user_code: string; verification_uri: string; login_id: string } | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  let alive = true;
  onCleanup(() => { alive = false; });
  const start = async () => {
    if (busy()) return;
    setBusy(true); setError(null);
    try {
      const r = await api.machine<{ login_id: string; user_code: string; verification_uri: string }>(p.server, "deck.github.login.start");
      setCode(r);
      await api.openExternal(r.verification_uri).catch(() => { /* the link is shown */ });
      for (;;) {
        await sleep(2000);
        if (!alive) return;
        const s = await api.machine<{ state: string; user?: string | null; message?: string | null }>(p.server, "deck.github.login.status", { login_id: r.login_id });
        if (s.state === "done") { p.onDone(s.user ?? null); return; }
        if (s.state !== "pending") throw new Error(s.message || "the sign-in did not finish");
      }
    } catch (e) {
      if (alive) { setError(String(e).replace(/^Error: /, "").replace(/^the server refused deck\.github\.[\w.]+: /, "")); setCode(null); }
    } finally {
      if (alive) setBusy(false);
    }
  };
  const copy = () => { const c = code(); if (c) api.clipboardWriteText(c.user_code).catch(() => {}); };
  return (
    <div class="chat-menu ask signin gh">
      <div class="cm-title">▲ SIGN IN TO GITHUB{p.where ? ` ON ${p.where.toUpperCase()}` : ""}</div>
      <Show when={code()} fallback={
        <>
          <div class="ask-head">{p.reason ?? "This machine could not reach the repository with the credentials it has."} Sign in to GitHub once on {p.where || "this machine"}; the token stays there and is used only for git over HTTPS.</div>
          <Show when={error()}>{(e) => <div class="repo-error"><span>{e()}</span></div>}</Show>
          <div class="cm-foot">
            <button class="abtn hot" disabled={busy()} onClick={() => void start()}>{busy() ? "STARTING…" : "SIGN IN TO GITHUB"}</button>
            <Show when={p.onCancel}><button class="abtn" disabled={busy()} onClick={() => p.onCancel?.()}>CANCEL</button></Show>
            <span class="sp" />
          </div>
        </>
      }>
        {(c) => (
          <>
            <div class="ask-head">GitHub opened in your browser. Enter this code there and approve; the clone goes on by itself.</div>
            <div class="gh-code" title="click to copy" onClick={copy}>{c().user_code}</div>
            <div class="signin-link"><a href={c().verification_uri}>{c().verification_uri}</a></div>
            <div class="cm-foot">
              <span class="micro">waiting for GitHub…</span>
              <button class="abtn" onClick={copy}>COPY CODE</button>
              <Show when={p.onCancel}><button class="abtn" onClick={() => { alive = false; p.onCancel?.(); }}>CANCEL</button></Show>
              <span class="sp" />
            </div>
          </>
        )}
      </Show>
    </div>
  );
}
