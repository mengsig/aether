// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { archonHarness, type ArchonHarness } from "../queue/test/harness.tsx";
import PipelineRoute from "./PipelineScreen.tsx";

let h: ArchonHarness | null = null;
beforeEach(() => { vi.useFakeTimers({ now: Date.parse("2026-11-04T10:00:00Z") }); });
afterEach(() => {
  h?.stop();
  h = null;
  document.body.innerHTML = "";
  vi.useRealTimers();
});

const steps = (container: HTMLElement) => [...container.querySelectorAll(".pipeline-step")].map((step) =>
  `${step.querySelector(".pipeline-name")?.textContent ?? ""}: ${step.querySelector(".pipeline-step-head .ui-pill")?.textContent ?? ""}`);

async function pipeline(item: string, run: string) {
  h = await archonHarness({ protocol: 24 });
  const container = await h.mount(<PipelineRoute params={{ item, run }} />);
  await h.until(() => container.querySelectorAll(".pipeline-step").length > 0, "the pipeline steps");
  return { h, container };
}

test("draws an edited run_plan with each stage's runs and the current stage", async () => {
  const { h, container } = await pipeline("qi_dark_mode", "run_dark");
  expect(h.faux.commands("run_plan")).toEqual([expect.objectContaining({ run: "run_dark" })]);
  expect(steps(container)).toEqual(["plan: Completed", "build: Completed", "review: Running", "docs: Not started", "ship: Not started"]);
  expect(container.querySelector(".pipeline-note")?.textContent).toBe("Plan revision r2 Edited after it started");
  const review = container.querySelector('.pipeline-step[aria-current="step"]');
  expect(review?.querySelector(".pipeline-name")?.textContent).toBe("review");
  expect(review?.querySelector(".pipeline-terms")?.textContent).toBe("review · gate all pass · 2 in parallel · loops to build (max 2) · 60 min limit");
  expect(review?.querySelectorAll(".item-stage")).toHaveLength(2);
  expect(container.querySelector(".item-back")?.getAttribute("href")).toBe("#/archon/items/qi_dark_mode");
});

test("marks the gate a run waits on and the stages a finished run never ran", async () => {
  const waiting = await pipeline("qi_pairing", "run_pair");
  expect(steps(waiting.container)).toEqual(["plan: Needs you", "build: Not started", "review: Not started", "ship: Not started"]);
  h?.stop();
  document.body.innerHTML = "";
  const failed = await pipeline("qi_assets", "run_assets");
  expect(steps(failed.container)).toEqual(["plan: Completed", "build: Timed out", "review: Not run", "ship: Not run"]);
});

test("follows the change stream as the run moves on", async () => {
  const { h, container } = await pipeline("qi_dark_mode", "run_dark");
  const entry = h.world.items.get("qi_dark_mode");
  if (!entry) throw new Error("the demo world has qi_dark_mode");
  const stages = entry.detail.stages as Record<string, unknown>[];
  entry.detail = { ...entry.detail, item: { ...(entry.detail.item as object), updated_at: "2026-11-04T10:01:00Z" },
    stages: stages.map((stage) => (stage.stage_id === "review" ? { ...stage, state: { state: "completed" }, verdict: "pass" } : stage)) };
  h.faux.tell("qi_dark_mode", { subject: "item", item: "qi_dark_mode", state: "running", stage: "docs" });
  await h.until(() => steps(container)[2] === "review: Completed", "the finished review");
  expect(h.faux.commands("run_plan").length).toBeGreaterThanOrEqual(2);
});

test("says when the item has no such run", async () => {
  h = await archonHarness({ protocol: 24 });
  const container = await h.mount(<PipelineRoute params={{ item: "qi_dark_mode", run: "run_missing" }} />);
  await h.until(() => container.querySelector(".ui-state-title") !== null, "the missing run");
  expect(container.querySelector(".ui-state-title")?.textContent).toBe("Run not found");
});
