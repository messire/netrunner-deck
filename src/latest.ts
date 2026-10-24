/**
 * "Last request wins" per key. Call `begin(key)` before an async request and
 * keep the returned sequence number; after the `await`, apply the result only
 * if `isCurrent(key, seq)` — any newer `begin` for the same key invalidates it.
 */
export interface LatestGate {
  begin(key: string): number;
  isCurrent(key: string, seq: number): boolean;
}

export function createLatestGate(): LatestGate {
  const seqs = new Map<string, number>();
  return {
    begin(key) {
      const seq = (seqs.get(key) ?? 0) + 1;
      seqs.set(key, seq);
      return seq;
    },
    isCurrent(key, seq) {
      return seqs.get(key) === seq;
    },
  };
}
