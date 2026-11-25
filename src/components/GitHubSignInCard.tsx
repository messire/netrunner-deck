import { Show, createSignal, onCleanup } from "solid-js";
import { api } from "../api";
import { sleep } from "../jobs";

interface LoginStatus { state: string; user_code?: string | null; verification_uri?: string | null; user?: string | null; message?: string | null }

/**
 * Sign in to GitHub on a server's machine (deck-server's `deck.github.*`):
 * the server runs the official GitHub CLI (`gh auth login --web`, getting
 * `gh` first if the machine has none), Deck shows the one-time code and
 * opens the link in the browser, the user approves as any GitHub user,
 * `gh` keeps the login on that machine and becomes git's credential helper
 * for github.com there. Deck never sees a token.
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
  const [st, setSt] = createSignal<LoginStatus | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  let alive = true;
  onCleanup(() => { alive = false; });
  const start = async () => {
    if (busy()) return;
    setBusy(true); setError(null);
    try {
      const { login_id } = await api.machine<{ login_id: string }>(p.server, "deck.github.login.start");
      setSt({ state: "installing" });
      let opened = false;
      for (;;) {
        if (!alive) return;
        const s = await api.machine<LoginStatus>(p.server, "deck.github.login.status", { login_id });
        if (!alive) return;
        setSt(s);
        if (s.user_code && s.verification_uri && !opened) { opened = true; api.openExternal(s.verification_uri).catch(() => { /* the link is shown */ }); }
        if (s.state === "done") { p.onDone(s.user ?? null); return; }
        if (s.state === "failed") throw new Error(s.message || "the sign-in did not finish");
        await sleep(1500);
      }
    } catch (e) {
      if (alive) { setError(String(e).replace(/^Error: /, "").replace(/^the server refused deck\.github\.[\w.]+: /, "")); setSt(null); }
    } finally {
      if (alive) setBusy(false);
    }
  };
  const copy = () => { const c = st()?.user_code; if (c) api.clipboardWriteText(c).catch(() => {}); };
  const where = () => p.where || "this machine";
  const link = () => st()?.verification_uri ?? "https://github.com/login/device";
  const cancel = () => { alive = false; p.onCancel?.(); };
  return (
    <div class="chat-menu ask signin gh">
      <div class="cm-title">▲ SIGN IN TO GITHUB{p.where ? ` ON ${p.where.toUpperCase()}` : ""}</div>
      <Show when={st()} fallback={
        <>
          <div class="ask-head">{p.reason ? `${p.reason} ` : ""}Sign in to GitHub once on {where()} with the GitHub CLI; git there (clones, Claude's commands) then reaches your repositories.</div>
          <Show when={error()}>{(e) => <div class="repo-error"><span>{e()}</span></div>}</Show>
          <div class="cm-foot">
            <button class="abtn hot" disabled={busy()} onClick={() => void start()}>{busy() ? "STARTING…" : "SIGN IN TO GITHUB"}</button>
            <Show when={p.onCancel}><button class="abtn" disabled={busy()} onClick={() => p.onCancel?.()}>CANCEL</button></Show>
            <span class="sp" />
          </div>
        </>
      }>
        {(s) => (
          <Show when={s().user_code} fallback={
            <>
              <div class="ask-head">{s().state === "installing" ? `Getting the GitHub CLI on ${where()}…` : "Starting the GitHub CLI…"}</div>
              <div class="cm-foot"><span class="micro">one moment</span><Show when={p.onCancel}><button class="abtn" onClick={cancel}>CANCEL</button></Show><span class="sp" /></div>
            </>
          }>
            <div class="ask-head">GitHub opened in your browser. Enter this code there and approve; Deck goes on by itself.</div>
            <div class="gh-code" title="click to copy" onClick={copy}>{s().user_code}</div>
            <div class="signin-link"><a href={link()}>{link()}</a></div>
            <div class="cm-foot">
              <span class="micro">waiting for GitHub…</span>
              <button class="abtn" onClick={copy}>COPY CODE</button>
              <Show when={p.onCancel}><button class="abtn" onClick={cancel}>CANCEL</button></Show>
              <span class="sp" />
            </div>
          </Show>
        )}
      </Show>
    </div>
  );
}
