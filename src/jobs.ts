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
