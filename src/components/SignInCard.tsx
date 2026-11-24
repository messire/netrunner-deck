import { Show, createSignal } from "solid-js";
import { api, type AccountList } from "../api";

/**
 * Sign in to Claude on a server's machine (deck-server's
 * `deck.accounts.login.*`): the server starts Claude Code's OAuth sign-in,
 * Deck opens the link in the browser, the user pastes the code the page
 * shows, the server exchanges it and keeps the account in its own store.
 * Used in the chat (the machine has no usable account) and by ACCOUNTS'
 * "+ ADD ACCOUNT".
 */
export default function SignInCard(p: {
  server: string | null;
  /** Machine name for the title. */
  where?: string;
  /** Make the new account the machine's login even when it has one. */
  activate?: boolean;
  title?: string;
  /** What the card says before the sign-in starts. */
  intro?: string;
  onDone: (list: AccountList) => void;
  onCancel?: () => void;
}) {
  const [login, setLogin] = createSignal<{ id: string; url: string } | null>(null);
  const [code, setCode] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const run = async (work: () => Promise<void>) => {
    if (busy()) return;
    setBusy(true);
    setError(null);
    try { await work(); } catch (e) { setError(String(e)); } finally { setBusy(false); }
  };
  const start = () => run(async () => {
    const r = await api.accountsLoginStart(p.server);
    setLogin({ id: r.login_id, url: r.url });
    setCode("");
    await api.openExternal(r.url).catch(() => { /* the link is shown to copy */ });
  });
  const finish = () => run(async () => {
    const l = login();
    if (!l || !code().trim()) return;
    const list = await api.accountsLoginFinish(p.server, l.id, code().trim(), p.activate ?? false);
    setLogin(null);
    p.onDone(list);
  });
  const copy = () => { const l = login(); if (l) api.clipboardWriteText(l.url).catch(() => {}); };
  return (
    <div class="chat-menu ask signin">
      <div class="cm-title">▲ {p.title ?? "SIGN IN TO CLAUDE"}{p.where ? ` · ${p.where.toUpperCase()}` : ""}</div>
      <Show when={login()} fallback={
        <>
          <div class="ask-head">{p.intro ?? "This machine has no Claude account to run the session. Sign in once; the account stays on this machine."}</div>
          <div class="cm-foot">
            <button class="abtn hot" disabled={busy()} onClick={start}>{busy() ? "STARTING…" : "SIGN IN"}</button>
            <Show when={p.onCancel}><button class="abtn" disabled={busy()} onClick={() => p.onCancel?.()}>CANCEL</button></Show>
            <span class="sp" />
          </div>
        </>
      }>
        {(l) => (
          <>
            <div class="ask-head">The sign-in page opened in your browser. Sign in there, then paste the code it shows.</div>
            <div class="signin-link"><a href={l().url} title={l().url}>{l().url}</a></div>
            <input class="ask-own" placeholder="paste the code here (Enter)" value={code()} disabled={busy()} autofocus
              onInput={(e) => setCode(e.currentTarget.value)}
              onKeyDown={(e) => { e.stopPropagation(); if (e.key === "Enter") void finish(); }} />
            <div class="cm-foot">
              <button class="abtn hot" disabled={busy() || !code().trim()} onClick={finish}>{busy() ? "SIGNING IN…" : "FINISH"}</button>
              <button class="abtn" disabled={busy()} onClick={copy}>COPY LINK</button>
              <button class="abtn" disabled={busy()} onClick={start}>NEW LINK</button>
              <Show when={p.onCancel}><button class="abtn" disabled={busy()} onClick={() => p.onCancel?.()}>CANCEL</button></Show>
              <span class="sp" />
            </div>
          </>
        )}
      </Show>
      <Show when={error()}>{(e) => <div class="chat-note err">{e()}</div>}</Show>
    </div>
  );
}
