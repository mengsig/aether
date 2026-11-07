// Journey 2, a task for an existing agent: Activity's start sheet in task mode, an owner chosen,
// and the task held by the daemon and shown on the owner's page.

import assert from "node:assert/strict";
import { expect, test } from "@playwright/test";
import { until } from "../../harness/process.ts";
import { agent, cancelLive, createAgentOn, taskObjectives } from "./daemon.ts";
import { connectionsReady, journeyHooks } from "./journey.ts";
import { shootWindow } from "./capture.ts";
import { agentPage, chooseWorkType, landOnAgent, objectiveField, openStart, startSheet, submitStart } from "./steps.ts";
import { need } from "./ui.ts";

const journey = journeyHooks({ archon: false });
const OWNER = "Journey owner";
const TASK = "Write the release note for the journey task.";
// The owner runs on the faux model in both variants, so its turns are free. It starts with no
// objective, so it idles holding no task and may take this one; its page then names the task.
const FAUX_MODEL = { provider: "mock", model: "mock-1" } as const;
const IDLE = new Set(["ready", "waiting"]);

test("task-for-existing-agent journey: a task in task mode lands on its owner", async () => {
  const { window, stack } = journey();
  await connectionsReady(journey(), false);
  const owner = await createAgentOn(stack, { name: OWNER, model: FAUX_MODEL, start: true });
  const idle = await until(() => agent(stack, owner), (view) => IDLE.has(view.state), "the owner to wait for work", 30_000);
  assert.ok(IDLE.has(idle.state), `the owner waits for work: ${idle.state}`);
  try {
    await openStart(window);
    const sheet = startSheet(window);
    await chooseWorkType(window, "Existing agent task");
    await (await objectiveField(window)).fill(TASK);
    const ownerChoice = await need(sheet, "start-owner");
    await ownerChoice.selectOption({ label: OWNER });
    await expect(ownerChoice, "the owner is chosen").toHaveValue(owner);
    await shootWindow(window, "start-agent", "task-mode");
    await submitStart(window, "Create task");
    const tasks = await until(() => taskObjectives(stack, owner), (objectives) => objectives.includes(TASK), "the task on its owner", 30_000);
    assert.equal(tasks.filter((objective) => objective === TASK).length, 1, "one task was created");
    await landOnAgent(window, owner);
    await expect(await need(agentPage(window), "agent-fact-task"), "the owner's page names its task").toContainText(TASK);
    await shootWindow(window, "agent", "task-found");
  } finally {
    await cancelLive(stack, [owner]);
  }
});
