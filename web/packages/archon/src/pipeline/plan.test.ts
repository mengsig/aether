import type { archon } from "@aether-ui/protocol";
import { describe, expect, test } from "vitest";
import { demoWorld } from "../queue/fixture/world.ts";
import { observedOrder, pipelineView } from "./plan.ts";

function fixture(item: string, run: string) {
  const world = demoWorld();
  const detail = world.items.get(item)?.detail as unknown as archon.ItemDetail | undefined;
  const found = detail?.runs.find((entry) => entry.id === run);
  if (!detail || !found) throw new Error(`the demo world has ${item}/${run}`);
  return { detail, run: found, plan: world.plans.get(run) as unknown as archon.RunPlan };
}

const summary = (view: ReturnType<typeof pipelineView>) => view.steps.map((step) => `${step.id}:${step.label}${step.current ? "*" : ""}`);

describe("a run's pipeline", () => {
  test("follows the plan, with a fan-out stage shown by its running slot", () => {
    const { detail, run, plan } = fixture("qi_dark_mode", "run_dark");
    const view = pipelineView(run, detail.stages, plan);
    expect(view).toMatchObject({ source: "plan", revision: "r2", edited: true, pipeline: "adaptive" });
    expect(summary(view)).toEqual(["plan:Completed", "build:Completed", "review:Running*", "docs:Not started", "ship:Not started"]);
    expect(view.steps[2]?.attempts).toHaveLength(2);
  });

  test("lists a stage that ran but left the plan after the planned ones", () => {
    const { detail, run, plan } = fixture("qi_dark_mode", "run_dark");
    const edited: archon.RunPlan = { ...plan, stages: plan.stages.filter((stage) => stage.id !== "build") };
    const view = pipelineView(run, detail.stages, edited);
    expect(view.steps.map((step) => [step.id, step.planned === null])).toEqual([["plan", false], ["review", false], ["docs", false], ["ship", false], ["build", true]]);
  });

  test("shows the latest attempt of a looped stage", () => {
    const { detail, run, plan } = fixture("qi_dark_mode", "run_dark");
    const first = detail.stages[1];
    if (!first) throw new Error("the build stage ran");
    const stages = [...detail.stages, { ...first, id: "sr_build_2", attempt: 2, state: { state: "failed", error: "tests failed" } } as archon.StageRun];
    expect(pipelineView(run, stages, plan).steps[1]?.label).toBe("Failed");
  });

  test("without a plan, lists the started stages in order and the current one last", () => {
    const { detail, run } = fixture("qi_dark_mode", "run_dark");
    expect(observedOrder(run, detail.stages)).toEqual(["plan", "build", "review"]);
    const later: archon.PipelineRun = { ...run, state: { state: "running", stage_id: "ship", attempt: 1 } };
    expect(summary(pipelineView(later, detail.stages, null))).toEqual(["plan:Completed", "build:Completed", "review:Running", "ship:Not started*"]);
  });

  test("keeps the frozen stage order and unrun stages for a completed run", () => {
    const { detail, run, plan } = fixture("qi_dark_mode", "run_dark");
    const finished: archon.PipelineRun = { ...run, state: { state: "succeeded", summary: "Done" } };
    expect(summary(pipelineView(finished, detail.stages, plan))).toEqual(["plan:Completed", "build:Completed", "review:Running", "docs:Not run", "ship:Not run"]);
  });

  test("refuses another run's plan", () => {
    const { detail, run, plan } = fixture("qi_dark_mode", "run_dark");
    expect(() => pipelineView(run, detail.stages, { ...plan, run: "run_other" })).toThrow(/not the plan of run_dark/);
  });
});
