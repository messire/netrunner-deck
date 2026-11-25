import { api, type MachineJob } from "./api";

/**
 * Long work on a server's machine runs there as a job (a clone, a
 * transfer's export and import, the removal after a move). These loops
 * ask about it until it ends; they belong to no dialog, so closing one
 * leaves the work going and its last step (opening what arrived) still
 * happens.
 */

export const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

/** Polls `status` until the job is no longer running; the last status. */
export async function waitJob(status: () => Promise<MachineJob>, onStatus: (s: MachineJob) => void = () => {}, every = 400): Promise<MachineJob> {
  for (;;) {
    const s = await status();
    onStatus(s);
    if (s.state !== "running") return s;
    await sleep(every);
  }
}

/** A job that did not end done is an error with the machine's words. */
export function ended(s: MachineJob, what: string): MachineJob {
  if (s.state === "done") return s;
  throw new Error(s.message || `${what} ${s.state ?? "failed"}`);
}

export interface Step { phase: string; percent: number | null }

export interface TransferRun {
  kind: "repo" | "harness";
  /** Source and target server ids. */
  from: string;
  to: string;
  fromName: string;
  toName: string;
  exportParams: Record<string, unknown>;
  /** A repository's new folder on the target (`~/name`). */
  path?: string;
  onConflict?: "overwrite" | "skip" | "keep_both";
  move: boolean;
  removeParams?: Record<string, unknown>;
  /** MOVE, after the target has it all: close what runs on the source. */
  beforeRemove?: () => Promise<void>;
  onStep: (s: Step) => void;
}

/** The overall percent of a step that spans `[from, to]` of the run. */
export const span = (from: number, to: number, pct: number | null | undefined) =>
  Math.round(from + ((to - from) * Math.max(0, Math.min(100, pct ?? 0))) / 100);

/**
 * COPY / MOVE: pack on the source, relay to the target, unpack there; a
 * move then removes the moved things from the source (only after the
 * target said it is all there). Resolves with the target's final status
 * (`path`, `worktrees`, `sessions`, or harness `copied` / `skipped` / `renamed`).
 */
export async function runTransfer(o: TransferRun): Promise<MachineJob> {
  const t = "deck.transfer.";
  const step = (phase: string, percent: number | null) => o.onStep({ phase, percent });
  step(`Packing on ${o.fromName}`, 0);
  const { job: exportJob } = await api.machine<{ job: string }>(o.from, `${t}export.start`, o.exportParams);
  let importJob: string | null = null;
  try {
    const packed = ended(await waitJob(() => api.machine(o.from, `${t}status`, { job: exportJob }),
      (s) => step(`${s.phase || "Packing"} on ${o.fromName}`, span(0, 25, s.percent))), "packing");
    const size = Number(packed.size ?? 0);
    const started = await api.machine<{ job: string; path?: string }>(o.to, `${t}import.start`, { kind: o.kind, ...(o.path ? { path: o.path } : {}) });
    importJob = started.job;
    step(`Sending to ${o.toName}`, 25);
    const { relay } = await api.transferRelay(o.from, o.to, exportJob, importJob);
    for (;;) {
      const r = await api.transferRelayStatus(relay);
      step(`Sending to ${o.toName} · ${mb(r.sent)} of ${mb(r.size || size)}`, span(25, 75, r.size ? (100 * r.sent) / r.size : 0));
      if (r.error) throw new Error(r.error);
      if (r.done) break;
      await sleep(300);
    }
    await api.machine(o.to, `${t}import.commit`, { job: importJob, size, ...(o.onConflict ? { on_conflict: o.onConflict } : {}) });
    const arrived = ended(await waitJob(() => api.machine(o.to, `${t}status`, { job: importJob! }),
      (s) => step(`${s.phase || "Unpacking"} on ${o.toName}`, span(75, o.move ? 88 : 100, s.percent))), "unpacking");
    await api.machine(o.from, `${t}finish`, { job: exportJob }).catch(() => {});
    if (o.move && o.removeParams) {
      step(`Removing from ${o.fromName}`, 90);
      await o.beforeRemove?.();
      const { job } = await api.machine<{ job: string }>(o.from, `${t}remove.start`, o.removeParams);
      ended(await waitJob(() => api.machine(o.from, `${t}status`, { job }), (s) => step(`${s.phase || "Removing"} on ${o.fromName}`, span(90, 100, s.percent))), "removing");
    }
    step("Done", 100);
    return arrived;
  } catch (e) {
    void api.machine(o.from, `${t}cancel`, { job: exportJob }).catch(() => {});
    if (importJob) void api.machine(o.to, `${t}cancel`, { job: importJob }).catch(() => {});
    throw e;
  }
}

export const mb = (n: number) => (n < 1024 * 1024 ? `${Math.max(0, Math.round(n / 1024))} KB` : `${(n / 1024 / 1024).toFixed(1)} MB`);
