/* @refresh reload */
import { ErrorBoundary } from "solid-js";
import { render } from "solid-js/web";
import "@xterm/xterm/css/xterm.css";
import "./styles.css";
import App from "./App";

// Last line of defence: an uncaught render error shows its text instead of a blank window.
const fault = (err: unknown) => (
  <div style="height:100%;display:grid;place-items:center;padding:24px;text-align:center;background:var(--bg);color:var(--red);font:11px Consolas,monospace;letter-spacing:1px;user-select:text">
    ◇ DECK FAULT — {err instanceof Error ? err.message : String(err)}
  </div>
);

render(
  () => (
    <ErrorBoundary fallback={fault}>
      <App />
    </ErrorBoundary>
  ),
  document.getElementById("root")!,
);
