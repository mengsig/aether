// Archon reads as core stores them, for the bundle tests and the screenshot page: one shipped item
// whose run passed review and tests behind an approved gate, and a failed one.

import type { archon } from "@aether-ui/protocol";

export const ARCHON_CONN = "archon:faux-understand";

const at = (minute: number): string => new Date(Date.UTC(2026, 10, 4, 9, minute, 0)).toISOString();

const choice = { effort: "medium", model: { provider: "openai", model: "gpt-6-astra" } } as archon.ModelChoice;

function stage(id: string, stageId: string, kind: archon.StageKind, state: archon.StageState, verdict: archon.Verdict | null, summary: string, attempt = 1): archon.StageRun {
  return { id, run: "run_cache", stage_id: stageId, kind, attempt, state, verdict, summary, choice, cost_usd: 0.4, fan_index: 0, started_at: at(attempt * 5), finished_at: at(attempt * 5 + 4) };
}

export const SHIPPED: archon.ItemDetail = {
  item: {
    id: "qi_cache", title: "Cache the dependency graph between builds", pipeline: "adaptive", priority: "high", tags: [], dependencies: [],
    brief: "Builds recompute the dependency graph from scratch. Cache it keyed by the lockfile hash and invalidate on change.\n\nAcceptance: a warm build skips graph resolution; a lockfile change rebuilds it.",
    created_at: at(0), updated_at: at(40), state: { state: "succeeded", run: "run_cache", summary: "The graph is cached under the lockfile hash; warm builds are 38% faster." },
    model_plan: {} as archon.ModelPlan,
  },
  runs: [{
    id: "run_cache", item: "qi_cache", pipeline: "adaptive", branch: "archon/cache-graph", workspace: "/w/cache", cost_usd: 2.1,
    started_at: at(1), finished_at: at(40), shipped: true, state: { state: "succeeded", summary: "Merged as pull request 212 after review and tests." },
  }],
  stages: [
    stage("sr_plan", "plan", "propose", { state: "completed" }, null, "Hash the lockfile, store the graph beside the build cache, drop it when the hash moves."),
    stage("sr_build", "build", "work", { state: "completed" }, null, "Added GraphCache with a lockfile-hash key and an invalidation test."),
    stage("sr_review", "review", "review", { state: "completed" }, "pass", "The key covers the lockfile and the toolchain version; no stale-read path found."),
    stage("sr_test", "test", "test", { state: "completed" }, "pass", "142 tests passed, including the warm and the invalidated build."),
  ],
  gates: [{
    item: "qi_cache", run: "run_cache", stage_id: "review", attempt: 1, subject: "report", opened_at: at(30), decided_at: at(32),
    decision: { decision: "approve", note: "Ship it." }, summary: "Review passed; the operator approved shipping.",
  }],
  spent_usd: 2.1,
};

export const SHIPPED_SUMMARY = {
  id: "qi_cache", title: SHIPPED.item.title, pr: { host: "github.com", kind: "pull_request", repo: "acme/forge", number: 212, state: "merged", review_decision: "approved" },
  pr_url: "https://github.com/acme/forge/pull/212",
} as unknown as archon.ItemSummary;

/** A failed item whose runs' errors together run past the 32 KiB a bundle may carry. */
export function oversized(): archon.ItemDetail {
  const runs = Array.from({ length: 20 }, (_, index): archon.PipelineRun => ({
    id: `run_big_${index}`, item: "qi_big", pipeline: "adaptive", branch: "archon/big", workspace: "/w/big", cost_usd: 1, started_at: at(index),
    state: { state: "failed", stage_id: "build", error: `Attempt ${index}: ${"the linker ran out of memory while ".repeat(80)}` },
  }));
  return { ...SHIPPED, item: { ...SHIPPED.item, id: "qi_big", title: "Make the linker fit in memory", state: { state: "failed", error: "out of memory" } }, runs, stages: [], gates: [] };
}

export const PR_OBJECTS: archon.GithubObject[] = [
  {
    ref: { host: "github.com", kind: "pull_request", repo: "acme/forge", number: 212 }, title: "Cache the dependency graph between builds",
    state: "merged", review_decision: "approved", checks: { state: "success", failed: 0, total: 9 }, draft: false, labels: ["build"],
    author: "mira", fetched_at: at(50), updated_at: at(41),
  },
  {
    ref: { host: "github.com", kind: "pull_request", repo: "acme/forge", number: 230 }, title: "Speed up cold builds",
    state: "open", review_decision: "review_required", checks: { state: "pending", failed: 0, total: 9 }, draft: true, labels: [],
    fetched_at: at(50), updated_at: at(48),
  },
];
