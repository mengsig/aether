// Opt-in real-engine acceptance journey. Run only through the private packaged journey runner.
import { expect, test } from "@playwright/test";
import { access, readFile } from "node:fs/promises";
import { REAL_MODELS } from "../../harness/real-engine.ts";
import { until } from "../../harness/process.ts";
import { connectionsReady, journeyHooks } from "./journey.ts";
import { assistantText } from "./daemon.ts";
import { assertModel, assertQueuedTurn, attachSlowRequests, capture, checkBinaries, configure, endStreamProbe, holdEngine, turnFinished, sampleReads, session, startSession, warmBudget, WAIT_MS } from "./learn-real-support.ts";
import { need, textField } from "./ui.ts";

const real = process.env.JOURNEY_ENGINE === "real";
const GOAL = "Learn simple linear equations. Ask me a recall question about solving 2x = 8.";

test.beforeAll(async () => {
  test.skip(!real, "opt-in real-engine journey");
  await checkBinaries();
});

const journey = journeyHooks({ archon: false });

test("packaged Learn with Haiku and Luna", async () => {
  test.setTimeout(40 * 60_000);
  const run = journey();
  const { stack, window } = run;
  const failures: unknown[] = [];
  for (const model of REAL_MODELS) {
    const name = `${model.provider}/${model.model}`;
    let stopReads: (() => Promise<void>) | undefined;
    try {
      await connectionsReady(run, false);
      await configure(stack, name, 5);
      await warmBudget(stack);
      const id = await startSession(window, "Learn something new", GOAL, true);
      stopReads = sampleReads(stack, id);
      await assertModel(stack, id, name);
      const question = window.getByRole("article", { name: "Recall question" }).filter({ has: window.getByTestId("learn-answer") }).last();
      await expect(question, `${name} poses a recall question`).toBeVisible({ timeout: WAIT_MS });
      await endStreamProbe(window, model.model);
      await capture(window, `${model.model}-question`);
      await configure(stack, name, 0.01);
      await (await textField(await need(question, "learn-answer"))).fill("x = 4");
      await question.getByText("Add how you got there").click();
      await question.getByLabel("Your reasoning").fill("Divide both sides by 2.");
      await question.getByRole("button", { name: "Answer", exact: true }).click();
      await until(() => session(stack, id), (value) => value.stalled?.reason === "budget_exhausted", "the recorded budget stall", WAIT_MS);
      await expect(window.locator('[data-kind="budget"]')).toBeVisible({ timeout: 30_000 });
      const saved = await session(stack, id);
      const answered = saved.questions?.find((entry) => entry.answer?.thinking === "Divide both sides by 2.");
      expect(answered, "the answer and its reasoning were recorded").toBeTruthy();
      await capture(window, `${model.model}-budget-stall`);
      await configure(stack, name, 5);
      await window.locator('[data-kind="budget"]').getByRole("button", { name: "Try again" }).click();
      const graded = await until(() => session(stack, id), (value) => value.questions?.some((entry) => entry.result?.correct != null) === true, "the recall grade", WAIT_MS);
      expect(graded.questions?.find((entry) => entry.answer?.thinking === "Divide both sides by 2.")?.result?.correct, "the teacher accepts the learner's reasoning").toBe(true);
      expect(await assistantText(stack, saved.agent)).not.toMatch(/didn.t come from you/i);
      await expect(window.getByRole("article", { name: "Recall question" }).locator('[data-correct="true"]')).toBeVisible({ timeout: WAIT_MS });
      await capture(window, `${model.model}-graded`);
      await until(async () => turnFinished(stack, id), Boolean, "the graded teaching turn to finish", WAIT_MS);
      const finishReads = stopReads;
      stopReads = undefined;
      await finishReads();
      await restartPractice(run, name, model.cli);
    } catch (error) {
      failures.push(new Error(`${name} journey failed`, { cause: error }));
    } finally {
      if (stopReads !== undefined) await stopReads().catch((error: unknown) => { failures.push(error); });
      await attachSlowRequests(stack, model.model).catch((error: unknown) => { failures.push(error); });
    }
  }
  if (failures.length > 0) throw new AggregateError(failures, "real Learn journey failed");
});

async function restartPractice(run: ReturnType<typeof journey>, model: string, cli: "claude" | "codex"): Promise<void> {
  const { stack, window } = run;
  const gate = await holdEngine(stack, cli);
  let released = false;
  try {
    await connectionsReady(run, false);
    const id = await startSession(window, "Practise what I know", "Practise solving simple linear equations, beginning with 2x = 8.");
    const before = await session(stack, id);
    expect(before.mode).toBe("practice");
    expect(before.active_problem, "practice has not yet produced its problem").toBeFalsy();
    await assertModel(stack, id, model);
    await until(() => access(gate.ready).then(() => true, () => false), Boolean, "practice turn to wait at the engine gate", WAIT_MS);
    const heldPid = await assertQueuedTurn(stack, id, gate.ready);
    await stack.aether.stop();
    await until(() => readFile(`/proc/${heldPid}/stat`, "utf8").then((stat) => stat.slice(stat.lastIndexOf(")") + 2).startsWith("Z "), () => true), Boolean, "the held engine to stop", 15_000);
    await gate.release();
    released = true;
    await stack.aether.start();
    await connectionsReady(run, false);
    await until(() => session(stack, id), (value) => value.active_problem != null, "practice to resume after restart", WAIT_MS);
    await until(async () => turnFinished(stack, id), Boolean, "the resumed turn to finish", WAIT_MS);
    await expect(window.getByRole("article", { name: "Practice problem" }).first()).toBeVisible({ timeout: WAIT_MS });
    await capture(window, `${model.split("/")[1]}-practice-resumed`);
  } finally {
    if (!released) await gate.release();
  }
}
