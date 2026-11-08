// Keeps an open graph current while extraction runs: the job list is reread while any job is in flight,
// and a job seen running that the list now reports finished rereads the graph, events or not.

import type { aether } from "@aether-ui/protocol";
import { useEffect, useRef } from "react";
import { useLearn, useNoesisRead } from "./data.ts";
import { invariant } from "./model.ts";

/** How often the job list is reread while a job is in flight. */
export const JOB_POLL_MS = 3_000;

const ACTIVE: ReadonlySet<aether.JobState> = new Set(["queued", "running", "merging"]);

/** The `noesis_jobs` reply, checked at the boundary; undefined before the first read. */
export function jobsOf(value: Record<string, unknown> | undefined): readonly aether.Job[] | undefined {
  if (value === undefined) return undefined;
  invariant(Array.isArray(value.jobs), "noesis_jobs carries jobs");
  const jobs = value.jobs as aether.Job[];
  invariant(jobs.every((job) => typeof job.id === "string" && typeof job.state === "string"), "every job names its id and state");
  return jobs;
}

/** The ids of jobs in `was` that `now` reports ended: their work is merged or abandoned. */
export function finishedSince(was: ReadonlySet<string>, now: readonly aether.Job[]): string[] {
  return now.filter((job) => was.has(job.id) && !ACTIVE.has(job.state)).map((job) => job.id);
}

export function useGraphFreshness(conn: string, reloadGraph: () => void): void {
  const jobsRead = useNoesisRead(conn, { kind: "jobs" });
  const jobs = jobsOf(useLearn(conn).reads.jobs?.value);
  const active = useRef<ReadonlySet<string>>(new Set());
  const inFlight = jobs?.some((job) => ACTIVE.has(job.state)) ?? false;
  useEffect(() => {
    if (jobs === undefined) return;
    if (finishedSince(active.current, jobs).length > 0) reloadGraph();
    active.current = new Set(jobs.filter((job) => ACTIVE.has(job.state)).map((job) => job.id));
  }, [jobs, reloadGraph]);
  useEffect(() => {
    if (!inFlight) return;
    const timer = setInterval(jobsRead.reload, JOB_POLL_MS);
    return () => clearInterval(timer);
  }, [inFlight, jobsRead.reload]);
}
