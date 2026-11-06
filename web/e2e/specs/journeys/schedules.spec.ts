// Journey 4, schedules: a weekday cron schedule that starts a new agent, listed by the daemon and
// the window, then deleted. Its agents run on the faux model in both variants, so it costs nothing.

import assert from "node:assert/strict";
import { expect, test, type Locator, type Page } from "@playwright/test";
import { until } from "../../harness/process.ts";
import { cancelLive, createAgentOn, scheduleNames } from "./daemon.ts";
import { connectionsReady, journeyHooks } from "./journey.ts";
import { openControls } from "./steps.ts";
import { choose, confirmIfAsked, finish, isTestId, need, textField } from "./ui.ts";

const journey = journeyHooks({ archon: false });
const NAME = "Journey weekday check";
const CRON = "0 9 * * 1-5";
const FAUX_MODEL = { provider: "mock", model: "mock-1" } as const;
const LIST_MS = 30_000;

/** A weekday-morning recurrence: the new UI's preset, else today's Cron expression. */
async function weekdayCron(page: Page, controls: Locator): Promise<void> {
  const preset = await need(page, "schedule-cron-preset", () => controls.getByLabel("Recurrence", { exact: true }));
  if (await isTestId(preset, "schedule-cron-preset")) {
    await choose(page, preset, /weekday|Mon.*Fri/i);
    return;
  }
  await choose(page, preset, "Cron");
  await controls.getByLabel("Cron expression", { exact: true }).fill(CRON);
}

/** The model for the schedule's agents: typed where the field is text, else picked by name. */
async function scheduleModel(page: Page, controls: Locator): Promise<void> {
  const found = await need(page, "schedule-model", () => controls.getByLabel("Model", { exact: true }));
  const tag = await found.evaluate((element) => element.tagName);
  if (tag === "INPUT" || tag === "TEXTAREA") await choose(page, found, `${FAUX_MODEL.provider}/${FAUX_MODEL.model}`);
  else await choose(page, found, /Faux model|mock/i);
}

async function fillSchedule(page: Page, controls: Locator, work: string): Promise<void> {
  await (await textField(await need(page, "schedule-name", () => controls.getByLabel("Schedule name", { exact: true })))).fill(NAME);
  const target = controls.getByLabel("Send to", { exact: true });
  if (await target.isVisible()) await choose(page, target, "A new agent");
  await weekdayCron(page, controls);
  await choose(page, await need(page, "schedule-timezone", () => controls.getByLabel("Timezone", { exact: true })), "UTC");
  await (await textField(await need(page, "schedule-workspace", () => controls.getByLabel("Workspace", { exact: true })))).fill(work);
  await scheduleModel(page, controls);
  const objective = controls.getByLabel("Objective for each new agent", { exact: true });
  if (await objective.isVisible()) await objective.fill("Check the workspace, then finish.");
}

test("schedules journey: create a weekday schedule, see it listed, delete it", async () => {
  const { window, stack } = journey();
  await connectionsReady(journey(), false);
  const host = await createAgentOn(stack, { name: "Journey scheduler", objective: "Hold the schedules.", model: FAUX_MODEL, start: false });
  try {
    const controls = await openControls(window, stack, host, "Schedules");
    await fillSchedule(window, controls, stack.layout.aether.work);
    await (await need(window, "schedule-submit", () => controls.getByRole("button", { name: "Create schedule", exact: true }))).click();
    await confirmIfAsked(window, "Create schedule");
    await until(() => scheduleNames(stack), (names) => names.includes(NAME), "the schedule on the daemon", LIST_MS);
    const all = controls.getByRole("button", { name: "All schedules" });
    if (await all.isVisible()) await all.click();
    const row = window.getByRole("listitem").filter({ hasText: NAME }).first();
    await expect(row, "the schedule is listed").toBeVisible();
    await row.getByRole("button", { name: /^(Delete|Cancel schedule)\b/ }).click();
    await confirmIfAsked(window, /^(Delete|Cancel schedule)\b/);
    const left = await until(() => scheduleNames(stack), (names) => !names.includes(NAME), "the schedule to be deleted", LIST_MS);
    assert.ok(!left.includes(NAME), "the daemon no longer runs the schedule");
  } finally {
    await cancelLive(stack, [host]);
  }
  finish();
});
