/**
 * The "working / thinking" glyph. Two renderings share one look — a glitching
 * red triangle that collapses into a square, a bar, a slash and back:
 *   vector — inline SVG morphing its path in CSS (crisp at any size, colourable)
 *   sprite — the original GIF cut into `assets/think-sprite.png` (kept for comparison)
 */
export const THINK_MODE: "vector" | "sprite" = "sprite";

export default function Think(p: { size?: number; tone?: "red" | "yellow" | "cyan" }) {
  const size = p.size ?? 28;
  // sprite + glitch tears: ::before/::after are copies of the same frame, clipped
  // to horizontal bands and shoved sideways for a few frames at a time.
  if (THINK_MODE === "sprite") return <span class={`think ${size <= 20 ? "sm" : ""}`} />;
  return (
    <svg class={`thinkv ${p.tone ?? "red"}`} width={size} height={size} viewBox="0 0 32 32" aria-hidden="true">
      {/* glow + two outlines (outer and inner), each a 4-point path so the morph interpolates */}
      <g class="glow"><path class="outer" /><path class="inner" /></g>
      <g class="ghost c"><path class="outer" /></g>
      <g class="ghost m"><path class="outer" /></g>
      <g class="line"><path class="outer" /><path class="inner" /></g>
    </svg>
  );
}
