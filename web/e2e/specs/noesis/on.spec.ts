// The control for the off specs: with Noesis and engineering context on, Learn from this is offered
// and the screens read, yet opening them still asks no model and starts no job. Turning Noesis on
// makes noesis.db, so this file sorts after the off-*.spec.ts files that assert it never appears.

import { createAgent, enqueueItem, pauseQueue } from "../../harness/agents.ts";
import { pairBoth } from "../../harness/pair.ts";
import { expect, test } from "../../harness/fixtures.ts";
import type { Stack } from "../../harness/stack.ts";
import { configureNoesis, entityActions, LEARN_FROM_THIS, noesisChanges, noesisCommands, noesisFiles, noesisStatus, openAgent, openItem,
  recordCommands, STARTERS } from "./noesis.ts";

const AGENT = "Noesis on agent";
const ITEM = "Noesis on item";

async function noesisJobs(stack: Stack): Promise<unknown[]> {
  const { reply, data } = await stack.aether.request({ cmd: "noesis_jobs" });
  expect(reply, "noesis_jobs answers while Noesis is on").toBe("noesis_jobs");
  expect(Array.isArray(data.jobs), `noesis_jobs lists jobs: ${JSON.stringify(data)}`).toBe(true);
  return data.jobs as unknown[];
}

test.beforeAll(async ({ stack }) => {
  await pauseQueue(stack);
  await enqueueItem(stack, ITEM);
  await createAgent(stack, AGENT, "Stay idle while Noesis is on.", false);
  const status = await configureNoesis(stack, { enabled: true, engineering_context: true });
  expect(status.effective.engineering_context, "engineering context is effective once Noesis is on").toBe(true);
  expect(await noesisFiles(stack), "turning Noesis on opens noesis.db").toContain("noesis.db");
});

test.afterAll(async ({ stack }) => {
  const status = await configureNoesis(stack, { enabled: false, engineering_context: false });
  expect(status.effective.engineering_context, "Noesis is back off for the specs after this file").toBe(false);
});

test("with Noesis on, opening the Learning screens reads them but asks no model and starts no job", async ({ page, stack }) => {
  const sent = recordCommands(page);
  await pairBoth(page, stack);
  const requests = stack.engine.requests.length;
  const from = sent.commands.length;

  await page.goto(`${stack.aether.origin}/#/learn`);
  await expect(page.getByRole("region", { name: "Your sessions" }).getByText("No sessions yet")).toBeVisible();
  for (const [screen, read] of [["#/learn/graph", "noesis_graph"], ["#/learn/me", "noesis_learner"]] as const) {
    await page.goto(`${stack.aether.origin}/${screen}`);
    await expect(page.getByTestId("noesis-off")).toHaveCount(0);
    await expect.poll(() => sent.commands.slice(from), { message: `${screen} reads with ${read}` }).toContain(read);
  }

  const during = sent.commands.slice(from);
  expect(noesisCommands(during), "the sessions screen read its list").toContain("noesis_list_sessions");
  expect(noesisChanges(during), "opening the screens sent no Noesis change").toEqual([]);
  expect(sent.unparsed, "every frame the page sent was a command").toEqual([]);
  expect(stack.engine.requests.length, "opening the screens asked no model").toBe(requests);
  expect(await noesisJobs(stack), "opening the screens created no Noesis job").toEqual([]);
  expect((await noesisStatus(stack)).jobs_running, "no Noesis job runs").toBe(0);
});

test("with engineering context on, Learn from this is offered on agents and items and opening it starts nothing", async ({ page, stack }, info) => {
  const sent = recordCommands(page);
  await pairBoth(page, stack);
  const requests = stack.engine.requests.length;
  const from = sent.commands.length;

  await openItem(page, stack, ITEM);
  expect(await entityActions(page, ".item-header"), "an item offers Learn from this").toContain(LEARN_FROM_THIS);
  await openAgent(page, stack, AGENT);
  expect(await entityActions(page, ".agent-head"), "an agent offers Learn from this").toContain(LEARN_FROM_THIS);
  await page.locator(".agent-head").getByRole("button", { name: "Actions", exact: true }).click();
  await page.getByRole("menuitem", { name: LEARN_FROM_THIS }).click();
  const sheet = page.getByRole("dialog", { name: LEARN_FROM_THIS });
  await expect(sheet, "Learn from this opens its sheet").toBeVisible();
  await expect(sheet.getByTestId("understand-off")).toHaveCount(0);
  await page.screenshot({ path: info.outputPath("learn-from-this.png") });

  const during = sent.commands.slice(from);
  expect(during.filter((cmd) => STARTERS.includes(cmd)), "opening Learn from this started no session, extraction or import").toEqual([]);
  expect(noesisChanges(during), "opening Learn from this sent no Noesis change").toEqual([]);
  expect(stack.engine.requests.length, "opening Learn from this asked no model").toBe(requests);
  expect(await noesisJobs(stack), "opening Learn from this created no Noesis job").toEqual([]);
});
