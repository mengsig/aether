import type { aether, archon } from "@aether-ui/protocol";
import { describe, expect, test } from "vitest";
import { agentBundle } from "./agent-bundle.ts";
import { gateStatus, itemBundle, prStatus, runBundle, runStatus, stageStatus } from "./archon-bundles.ts";
import { checkBundle } from "./bundle.ts";
import { oversized, PR_OBJECTS, SHIPPED, SHIPPED_SUMMARY } from "./fixture/evidence.ts";
import { findPr, parsePrId, prBundle } from "./pr-bundle.ts";

function present<T>(value: T | null | undefined): T {
  if (value === null || value === undefined) throw new Error("the fixture lacks a value this test needs");
  return value;
}

const GATE = present(SHIPPED.gates?.[0]);
const statuses = (bundle: aether.EvidenceBundle) => bundle.items.map((item) => `${item.ref.kind}:${item.ref.id}=${item.status}`);
const stage = (patch: Partial<archon.StageRun>): archon.StageRun => ({ ...SHIPPED.stages[0], ...patch } as archon.StageRun);
const run = (state: archon.RunState): archon.PipelineRun => ({ ...SHIPPED.runs[0], shipped: false, state } as archon.PipelineRun);

describe("status rules", () => {
  test("verified only for what a check or merge established", () => {
    expect(runStatus(run({ state: "succeeded", summary: "done" }))).toBe("verified");
    expect(stageStatus(stage({ kind: "test", state: { state: "completed" }, verdict: "pass" }))).toBe("verified");
    expect(prStatus("merged", null)).toBe("verified");
  });

  test("reviewed for a passed review or an approved gate", () => {
    expect(stageStatus(stage({ kind: "review", state: { state: "completed" }, verdict: "pass" }))).toBe("reviewed");
    expect(gateStatus({ ...GATE })).toBe("reviewed");
    expect(prStatus("open", "approved")).toBe("reviewed");
  });

  test("preliminary for briefs, agent output, running or failed work and open PRs", () => {
    expect(runStatus(run({ state: "running", stage_id: "build", attempt: 1 }))).toBe("preliminary");
    expect(runStatus(run({ state: "failed", stage_id: "build", error: "boom" }))).toBe("preliminary");
    expect(stageStatus(stage({ kind: "work", state: { state: "completed" }, verdict: null }))).toBe("preliminary");
    expect(stageStatus(stage({ kind: "review", state: { state: "completed" }, verdict: "fail" }))).toBe("preliminary");
    expect(stageStatus(stage({ kind: "test", state: { state: "running" }, verdict: "pass" }))).toBe("preliminary");
    expect(gateStatus({ ...GATE, decision: { decision: "revise", feedback: "again" } })).toBe("preliminary");
    expect(prStatus("open", "review_required")).toBe("preliminary");
  });
});

describe("the item and run bundles", () => {
  test("an item carries its outcome, brief, PR, runs, newest stages and gates with their statuses", () => {
    const bundle = itemBundle(SHIPPED, SHIPPED_SUMMARY);
    expect(bundle.title).toBe("Cache the dependency graph between builds");
    expect(statuses(bundle)).toEqual([
      "item:qi_cache=verified", "item:qi_cache=preliminary", "pr:acme/forge#212=verified", "run:run_cache=verified",
      "stage:sr_plan=preliminary", "stage:sr_build=preliminary", "stage:sr_review=reviewed", "stage:sr_test=verified", "stage:run_cache/review=reviewed",
    ]);
    expect(bundle.items[1]?.excerpt).toContain("Acceptance: a warm build skips graph resolution");
    expect(checkBundle(bundle).ok).toBe(true);
  });

  test("a run carries its plan and only its own stages' newest attempts", () => {
    const retried = { ...SHIPPED, stages: [...SHIPPED.stages, { ...present(SHIPPED.stages[1]), id: "sr_build_2", attempt: 2, summary: "Second try." }] };
    const plan = { run: "run_cache", pipeline: "adaptive", revision: "r1", edited: false, stages: [{ id: "plan" }, { id: "build" }] } as archon.RunPlan;
    const bundle = runBundle(retried, "run_cache", plan);
    expect(bundle.items[0]?.excerpt).toContain("Planned stages: plan → build");
    expect(statuses(bundle).filter((line) => line.startsWith("stage:sr_build"))).toEqual(["stage:sr_build_2=preliminary"]);
  });

  test("an oversized item is refused, not cut short", () => {
    const check = checkBundle(itemBundle(oversized(), null));
    expect(check.ok).toBe(false);
    expect(check.bytes).toBeGreaterThan(32_768);
  });
});

describe("the pull request bundle", () => {
  test("accepts Archon's key form and D4's route form, and nothing else", () => {
    const want = { host: "github.com", repo: "acme/forge", number: 212 };
    expect(parsePrId("github.com/acme/forge#212")).toEqual(want);
    expect(parsePrId("github.com/Acme/Forge/212")).toEqual(want);
    for (const bad of ["acme/forge#212", "github:github.com/acme/forge#212", "github.com/acme/forge#x", "github.com/acme/forge/pull/212", ""]) {
      expect(parsePrId(bad)).toBeNull();
    }
  });

  test("a merged PR is verified, its passing checks a separate verified fact", () => {
    const merged = present(findPr(PR_OBJECTS, { host: "github.com", repo: "acme/forge", number: 212 }));
    expect(statuses(prBundle(merged))).toEqual(["pr:github.com/acme/forge#212=verified", "pr:github.com/acme/forge#212=verified"]);
    expect(prBundle(merged).items[1]?.excerpt).toBe("All 9 checks passed");
  });

  test("an open draft with pending checks stays preliminary; an approved one is reviewed", () => {
    const open = present(PR_OBJECTS[1]);
    expect(statuses(prBundle(open))).toEqual(["pr:github.com/acme/forge#230=preliminary", "pr:github.com/acme/forge#230=preliminary"]);
    expect(prBundle(open).items[0]?.excerpt).toContain("State: draft");
    expect(prBundle({ ...open, review_decision: "approved", checks: null }).items.map((item) => item.status)).toEqual(["reviewed"]);
  });
});

describe("the agent bundle", () => {
  const agent = {
    id: "agt_1", name: "builder", role: "worker", model: { provider: "anthropic", model: "claude-opus-5-5" },
    config: { instructions: "Build the cache." }, status: { state: "completed", summary: "Cache built." },
  } as unknown as aether.Agent;
  const entry = (seq: number, role: aether.MessageRole, text: string): aether.TranscriptEntry =>
    ({ seq, at: "2026-11-04T09:00:00Z", invocation: null, message: { role, content: [{ type: "text", text }] } });

  test("quotes the agent's newest words, all preliminary", () => {
    const entries = [entry(1, "user", "Go."), entry(2, "assistant", "One."), entry(3, "assistant", "Two."), entry(4, "assistant", "Three."), entry(5, "assistant", "Four.")];
    const bundle = agentBundle(agent, entries);
    expect(bundle.items.map((item) => item.status)).toEqual(Array(5).fill("preliminary"));
    expect(bundle.items[0]?.excerpt).toContain("Status: completed: Cache built.");
    expect(bundle.items.slice(2).map((item) => item.excerpt)).toEqual(["The agent said:\nTwo.", "The agent said:\nThree.", "The agent said:\nFour."]);
  });
});
