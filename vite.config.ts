/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import solid from "vite-plugin-solid";
import { execSync } from "node:child_process";
import { readFileSync } from "node:fs";

// The build line at the bottom of the window: version, commit, build time.
const version = JSON.parse(readFileSync(new URL("./package.json", import.meta.url), "utf8")).version as string;
const git = (args: string) => { try { return execSync(`git ${args}`, { stdio: ["ignore", "pipe", "ignore"] }).toString().trim(); } catch { return ""; } };
const commit = process.env.DECK_BUILD_COMMIT || git("rev-parse --short=7 HEAD") || "unknown";
const dirty = !process.env.DECK_BUILD_COMMIT && git("status --porcelain --untracked-files=no") !== "";

// Tauri expects a fixed port and no HMR overlay noise in the webview.
export default defineConfig({
  plugins: [solid()],
  define: {
    __DECK_VERSION__: JSON.stringify(version),
    __DECK_COMMIT__: JSON.stringify(dirty ? `${commit}+` : commit),
    __DECK_BUILT__: JSON.stringify(new Date().toISOString()),
  },
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  envPrefix: ["VITE_", "TAURI_"],
  build: { target: "es2022", minify: "esbuild", sourcemap: true },
  test: {
    // Pure-logic modules only (dialogue, keys, events); no DOM, no xterm.
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
