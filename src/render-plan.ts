/**
 * Incremental pane rendering (M2): instead of `term.reset()` + a full repaint
 * on every snapshot, work out the smallest change between the logical lines
 * already in the xterm buffer and the new snapshot, so the chat scrolls like a
 * terminal: history stays, scroll position and selection survive, only the
 * tail that actually changed is rewritten.
 *
 * `prev` is everything the view has written so far (it may be longer than a
 * snapshot: lines that scrolled out of Herdr's `recent` window stay in the
 * xterm scrollback). `next` is the new snapshot after the view's own
 * processing (frames, recolouring). Both are logical lines, not buffer rows.
 */
export type UpdatePlan =
  /** Nothing changed. */
  | { kind: "none" }
  /** `next` continues `prev`: write `lines` after the last line. */
  | { kind: "append"; lines: string[] }
  /** Cut `prev` at `fromLine` (an index into `prev`) and write `lines` there. */
  | { kind: "rewrite"; fromLine: number; lines: string[] }
  /** No alignment found (first render, or the pane changed wholesale): repaint. */
  | { kind: "reset"; lines: string[] };

export interface PlanOptions {
  /** How far before the end of `prev` an alignment with the start of `next` may begin, beyond `next.length`. */
  maxShift?: number;
  /** Fewest equal lines that count as an alignment (guards against matching on blank lines). */
  minAnchor?: number;
}

const DEFAULTS: Required<PlanOptions> = { maxShift: 400, minAnchor: 3 };

/**
 * Pure. Finds the offset `k` in `prev` where `next` starts (the snapshot
 * window slides forward as history grows), measures how many lines from
 * there are unchanged, and returns what to do with the rest. Prefers the
 * longest match; among equal matches the latest offset (the snapshot is the
 * tail of the history, so a match near the end of `prev` is the likelier one).
 */
export function planUpdate(prev: readonly string[], next: readonly string[], options: PlanOptions = {}): UpdatePlan {
  const { maxShift, minAnchor } = { ...DEFAULTS, ...options };
  if (prev.length === 0) return next.length === 0 ? { kind: "none" } : { kind: "reset", lines: [...next] };
  if (next.length === 0) return { kind: "reset", lines: [] };

  // Candidate offsets: `next` starts at prev[k]. In steady state k = 0; once
  // the window has slid, k = prev.length - overlap. Blank lines are poor
  // anchors, so an alignment must begin on a non-blank line of `next` that is
  // matched, or match `next` entirely.
  const firstK = Math.max(0, prev.length - next.length - maxShift);
  let bestK = -1;
  let bestP = 0;
  for (let k = firstK; k < prev.length; k++) {
    let p = 0;
    const max = Math.min(prev.length - k, next.length);
    while (p < max && prev[k + p] === next[p]) p++;
    if (p > 0 && p >= bestP) { bestP = p; bestK = k; }
  }
  const anchored = bestP >= minAnchor || (bestP === next.length && bestP > 0) || (bestK >= 0 && bestP === prev.length - bestK && bestP > 0 && nonBlank(next, bestP));
  if (!anchored) return { kind: "reset", lines: [...next] };

  const fromLine = bestK + bestP; // first line of prev that no longer holds
  const tail = next.slice(bestP);
  if (fromLine >= prev.length) return tail.length === 0 ? { kind: "none" } : { kind: "append", lines: tail };
  return { kind: "rewrite", fromLine, lines: tail };
}

function nonBlank(lines: readonly string[], upTo: number): boolean {
  for (let i = 0; i < upTo; i++) if (lines[i].trim() !== "") return true;
  return false;
}
