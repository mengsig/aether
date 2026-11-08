// A repository with changes to catch up on: three merged PRs (one older than the range), items in each
// state, and the Aether memories filed under its GitHub scope.

import type { aether, archon } from "@aether-ui/protocol";

export const REPO: archon.Repo = { id: "repo_forge", name: "forge", path: "/src/forge", ship: "pr", added_at: "2026-09-01T00:00:00Z" } as archon.Repo;
export const OTHER_REPO: archon.Repo = { id: "repo_widget", name: "widget", path: "/src/widget", ship: "pr", added_at: "2026-09-02T00:00:00Z" } as archon.Repo;
export const SINCE = "2026-11-01";
export const MEMORY_SCOPE = "github.com/acme/forge";

const FORGE = { host: "github.com", repo: "acme/forge" };
const day = (text: string): string => `${text}T10:00:00Z`;

export function pull(number: number, title: string, updated: string, patch: Partial<archon.GithubObject> = {}): archon.GithubObject {
  return {
    ref: { ...FORGE, kind: "pull_request", number }, title, state: "merged", labels: [], fetched_at: day("2026-11-06"), updated_at: day(updated), ...patch,
  };
}

export const MERGED: archon.GithubObject[] = [
  pull(212, "Cache the dependency graph between builds", "2026-11-04", { author: "mira", labels: ["build"] }),
  pull(215, "Retry flaky registry fetches", "2026-11-02"),
  pull(190, "Drop the legacy resolver", "2026-10-20"),
];

export function summary(id: string, title: string, updated: string, patch: Partial<archon.ItemSummary> = {}): archon.ItemSummary {
  return {
    id, title, repo: REPO.id, pipeline: "adaptive", priority: "normal", tags: [], dependencies: [], brief: `Brief of ${title}.`, spent_usd: 1,
    created_at: day("2026-10-30"), updated_at: day(updated), state: { state: "running" }, ...patch,
  } as archon.ItemSummary;
}

export const PASSING = { state: "success", failed: 0, total: 9 } as const;

export const SUMMARIES: archon.ItemSummary[] = [
  summary("qi_cache", "Cache the dependency graph", "2026-11-04", {
    state: { state: "succeeded", run: "run_cache", summary: "Warm builds are 38% faster." } as archon.QueueState,
    pr: { ...FORGE, kind: "pull_request", number: 212, state: "merged", checks: PASSING } as archon.PrRef,
  }),
  summary("qi_retry", "Retry registry fetches", "2026-11-03", {
    state: { state: "succeeded", run: "run_retry", summary: "Three attempts with backoff." } as archon.QueueState,
    pr: { ...FORGE, kind: "pull_request", number: 215, state: "open", review_decision: "approved" } as archon.PrRef,
  }),
  summary("qi_lint", "Tighten the lint rules", "2026-11-05"),
  summary("qi_old", "Rename the cache module", "2026-10-15"),
];

const memory = (id: string, text: string, patch: Partial<aether.MemoryRecord> = {}): aether.MemoryRecord => ({
  id, text, kind: "decision", confidence: 0.9, importance: 0.5, created_at: day("2026-10-01"), scope: { scope: "repository", id: MEMORY_SCOPE },
  provenance: {}, ...patch,
});

export const MEMORIES: aether.MemoryRecord[] = [
  memory("mem_a", "Builds key their cache on the lockfile hash.", { importance: 0.4 }),
  memory("mem_b", "Never merge to main without the release check.", { pinned: true, kind: "constraint", importance: 0.3 }),
  memory("mem_c", "An old approach, replaced.", { superseded_by: "mem_a" }),
  memory("mem_d", "Forgotten note.", { forgotten_at: day("2026-10-10") }),
];
