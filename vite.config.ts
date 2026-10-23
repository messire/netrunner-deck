import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

// Tauri expects a fixed port and no HMR overlay noise in the webview.
export default defineConfig({
  plugins: [solid()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  envPrefix: ["VITE_", "TAURI_"],
  build: { target: "es2022", minify: "esbuild", sourcemap: true },
});
