// Journey 4, schedules: a weekday cron schedule that starts a new agent, listed by the daemon and
// the window, then deleted. Its agents run on the faux model in both variants, so it costs nothing.

import assert from "node:assert/strict";
import { expect, test, type Locator, type Page } from "@playwright/test";
import { until } from "../../harness/process.ts";
import { cancelLive, createAgentOn, scheduleNamed, scheduleNames, type SavedSchedule } from "./daemon.ts";
import { connectionsReady, journeyHooks } from "./journey.ts";
import { shootWindow } from "./capture.ts";
import { openControls } from "./steps.ts";
import { choose, confirmIfAsked, need, textField } from "./ui.ts";

const journey = journeyHooks({ archon: false });
const NAME = "Journey weekday check";
const CRON = "0 9 * * 1-5";
const FAUX_MODEL = { provider: "mock", model: "mock-1" } as const;
const LIST_MS = 30_000;

/** A weekday-morning cron recurrence, from the cron presets. */
async function weekdayCron(page: Page, controls: Locator): Promise<void> {
  await choose(page, controls.getByRole("radiogroup", { name: "Recurrence" }), "Cron");
  await choose(page, await need(controls, "schedule-cron-preset"), /weekday/i);
  await expect(controls.getByRole("status").filter({ hasText: "Every weekday at 09:00" }), `the form reads ${CRON} back`).toBeVisible();
}

async function fillSchedule(page: Page, controls: Locator, work: string): Promise<void> {
  await (await textField(await need(controls, "schedule-name"))).fill(NAME);
  await choose(page, controls.getByRole("radiogroup", { name: "Send to" }), "A new agent");
  await weekdayCron(page, controls);
  await choose(page, await textField(await need(controls, "schedule-timezone")), "UTC");
  const workspace = await textField(await need(controls, "schedule-workspace"));
  await workspace.fill(work);
  await expect(workspace, "the schedule's workspace").toHaveValue(work);
  await choose(page, await need(controls, "schedule-model"), "Faux model");
  await controls.getByLabel("Objective for each new agent", { exact: true }).fill("Check the workspace, then finish.");
}

/** What the form chose must be what the daemon holds: the weekday cron in UTC, spawning a new faux-model agent in the workspace. */
function checkSaved(saved: SavedSchedule, work: string): void {
  assert.deepEqual(saved.recurrence, { kind: "cron", expression: CRON, timezone: "UTC" }, "the saved recurrence is the weekday cron in UTC");
  assert.equal(saved.target ?? null, null, "a schedule for a new agent names no existing one");
  const spawn = saved.spawn;
  assert.ok(spawn, "the schedule spawns a new agent");
  assert.deepEqual(spawn.model, FAUX_MODEL, "the new agent runs on the chosen model");
  assert.equal(spawn.objective, "Check the workspace, then finish.", "the new agent gets the objective typed");
  assert.equal(spawn.scope?.workspace_root, work, "the new agent works in the workspace chosen");
}

test("schedules journey: create a weekday schedule, see it listed, delete it", async () => {
  const { window, stack } = journey();
  await connectionsReady(journey(), false);
  const host = await createAgentOn(stack, { name: "Journey scheduler", objective: "Hold the schedules.", model: FAUX_MODEL, start: false });
  try {
    const controls = await openControls(window, stack, host, "Schedules");
    await fillSchedule(window, controls, stack.layout.aether.work);
    const submit = await need(controls, "schedule-submit");
    await expect(submit, "the schedule is ready to create").toBeEnabled();
    await shootWindow(window, "controls", "schedules-form");
    await submit.click();
    await confirmIfAsked(window, "Create schedule");
    const saved = await until(() => scheduleNamed(stack, NAME), (found) => found !== undefined, "the schedule on the daemon", LIST_MS);
    checkSaved(saved as SavedSchedule, stack.layout.aether.work);
    await choose(window, controls.getByRole("radiogroup", { name: "Schedule scope" }), "All schedules");
    const row = controls.getByRole("listitem").filter({ hasText: NAME }).first();
    await expect(row, "the schedule is listed").toBeVisible();
    await shootWindow(window, "controls", "schedules-list");
    await row.getByRole("button", { name: "Cancel schedule", exact: true }).click();
    await confirmIfAsked(window, "Cancel schedule");
    const left = await until(() => scheduleNames(stack), (names) => !names.includes(NAME), "the schedule to be deleted", LIST_MS);
    assert.ok(!left.includes(NAME), "the daemon no longer runs the schedule");
  } finally {
    await cancelLive(stack, [host]);
  }
});
