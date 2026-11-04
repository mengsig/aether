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
  await h.until(() => container.querySelectorAll(".pipeline-step").length > 0 && container.querySelector(".pipeline-note") !== null, "the pipeline steps and their plan");
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

/** Answers `run_plan` through `answer`, after telling a stage change first on each call `overtakes` picks. */
function overtakePlans(harness: ArchonHarness, overtakes: (call: number) => boolean, answer?: (call: number) => unknown): void {
  const faux = harness.faux as unknown as { answer(command: Record<string, unknown>): unknown };
  const real = faux.answer.bind(harness.faux);
  let calls = 0;
  faux.answer = (command) => {
    if (command.cmd !== "run_plan") return real(command);
    calls += 1;
    // The change frame is queued before the reply, so it arrives while the read is in flight.
    if (overtakes(calls)) harness.faux.tell("qi_dark_mode", { subject: "stage", stage_run: `sr_x${calls}`, run: "run_dark", item: "qi_dark_mode", stage_id: "review", attempt: 1, fan_index: 1, state: "running" });
    return answer?.(calls) ?? real(command);
  };
}

const PLANNED = ["plan", "build", "review", "docs", "ship"];

test("reads the plan again when a change overtakes its first read", async () => {
  h = await archonHarness({ protocol: 24 });
  overtakePlans(h, (call) => call === 1);
  const container = await h.mount(<PipelineRoute params={{ item: "qi_dark_mode", run: "run_dark" }} />);
  await h.until(() => container.querySelector(".pipeline-note") !== null, "the plan after its overtaken read");
  expect(h.faux.commands("run_plan").length).toBeGreaterThanOrEqual(2);
  expect(container.querySelector(".pipeline-note")?.textContent).toBe("Plan revision r2 Edited after it started");
  expect(steps(container).map((step) => step.split(":")[0])).toEqual(PLANNED);
});

test("shows why the plan failed when its read again is refused", async () => {
  h = await archonHarness({ protocol: 24 });
  overtakePlans(h, (call) => call === 1, (call) => (call > 1 ? { err: { code: "internal", message: "plan store unreadable" } } : undefined));
  const container = await h.mount(<PipelineRoute params={{ item: "qi_dark_mode", run: "run_dark" }} />);
  await h.until(() => container.querySelector(".pipeline-note") !== null, "the refused plan's note", 5_000);
  expect(container.querySelector(".pipeline-header .ui-skeleton")).toBeNull();
  expect(container.querySelector(".pipeline-note")?.textContent).toContain("run_plan failed");
});

test("shows the plan when changes keep overtaking its reads", async () => {
  h = await archonHarness({ protocol: 24 });
  overtakePlans(h, () => true);
  const container = await h.mount(<PipelineRoute params={{ item: "qi_dark_mode", run: "run_dark" }} />);
  await h.until(() => container.querySelector(".pipeline-note") !== null, "the plan despite the changes", 5_000);
  expect(h.faux.commands("run_plan").length).toBeGreaterThanOrEqual(3);
  expect(steps(container).map((step) => step.split(":")[0])).toEqual(PLANNED);
});

test("says when the item has no such run", async () => {
  h = await archonHarness({ protocol: 24 });
  const container = await h.mount(<PipelineRoute params={{ item: "qi_dark_mode", run: "run_missing" }} />);
  await h.until(() => container.querySelector(".ui-state-title") !== null, "the missing run");
  expect(container.querySelector(".ui-state-title")?.textContent).toBe("Run not found");
});
