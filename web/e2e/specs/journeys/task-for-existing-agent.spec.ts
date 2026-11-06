// Journey 2, a task for an existing agent: Activity's start sheet in task mode, an owner chosen,
// and the task held by the daemon and shown on the owner's page.

import assert from "node:assert/strict";
import { expect, test } from "@playwright/test";
import { until } from "../../harness/process.ts";
import { aetherConn, route } from "../flows/desktop/state.ts";
import { cancelLive, createAgentOn, taskObjectives } from "./daemon.ts";
import { connectionsReady, journeyHooks } from "./journey.ts";
import { agentPage, objectiveField, openStart, startSheet, submitStart } from "./steps.ts";
import { choose, finish, need, shows } from "./ui.ts";

const journey = journeyHooks({ archon: false });
const OWNER = "Journey owner";
const TASK = "Write the release note for the journey task.";
// The owner never runs a turn: it waits unstarted on the faux model, in both variants.
const FAUX_MODEL = { provider: "mock", model: "mock-1" } as const;

test("task-for-existing-agent journey: a task in task mode lands on its owner", async () => {
  const { window, stack } = journey();
  await connectionsReady(journey(), false);
  const owner = await createAgentOn(stack, { name: OWNER, objective: "Wait for tasks.", model: FAUX_MODEL, start: false });
  try {
    await openStart(window);
    await choose(window, await need(window, "start-mode", (p) => startSheet(p).getByRole("tablist", { name: "Work type" })), /task/i);
    await (await objectiveField(window)).fill(TASK);
    await choose(window, await need(window, "start-owner", (p) => startSheet(p).getByRole("button", { name: OWNER, exact: true })), OWNER);
    await submitStart(window, "Create task");
    const tasks = await until(() => taskObjectives(stack, owner), (objectives) => objectives.includes(TASK), "the task on its owner", 30_000);
    assert.equal(tasks.filter((objective) => objective === TASK).length, 1, "one task was created");
    await route(window, `#/agents/${encodeURIComponent(await aetherConn(stack))}/${owner}`);
    await expect(agentPage(window), "the owner's page shows").toBeVisible();
    await shows(agentPage(window).getByText(TASK), "the task on the owner's page");
  } finally {
    await cancelLive(stack, [owner]);
  }
  finish();
});
