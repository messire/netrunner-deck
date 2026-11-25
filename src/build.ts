/** This build as the bottom bar shows it: `Deck 0.8.0 · 0ec7d31` (`· dev` under `tauri dev`). */
export const buildLine = (version: string, commit: string, dev: boolean) => `Deck ${version} · ${commit}${dev ? " · dev" : ""}`;
