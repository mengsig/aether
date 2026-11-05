// "Learn from this" on agents and items is offered only while Noesis is on and engineering
// context is effective: hidden by default, and hidden while it is stored on but Noesis is off.

import { createAgent, enqueueItem, pauseQueue } from "../../harness/agents.ts";
import { pairBoth } from "../../harness/pair.ts";
import { expect, test, type Page } from "../../harness/fixtures.ts";
import type { Stack } from "../../harness/stack.ts";
import { configureNoesis, entityActions, LEARN_FROM_THIS, noesisFiles, noesisStatus, openAgent, openItem, reconnect } from "./noesis.ts";

const AGENT = "Noesis off agent";
const ITEM = "Noesis off item";

/** The page has read Noesis off; capabilities follow that read. */
async function seesNoesisOff(page: Page, stack: Stack): Promise<void> {
  await page.goto(`${stack.aether.origin}/#/learn`);
  await expect(page.getByTestId("noesis-off"), "the page has read Noesis off").toBeVisible();
}

async function expectNoLearnFromThis(page: Page, stack: Stack, why: string): Promise<void> {
  await seesNoesisOff(page, stack);
  await openAgent(page, stack, AGENT);
  const agentActions = await entityActions(page, ".agent-head");
  expect(agentActions, `the agent menu is drawn (${why})`).toContain("Open controls");
  expect(agentActions, `no Learn from this on an agent (${why})`).not.toContain(LEARN_FROM_THIS);
  await openItem(page, stack, ITEM);
  expect(await entityActions(page, ".item-header"), `no Learn from this on an item (${why})`).not.toContain(LEARN_FROM_THIS);
  await expect(page.getByRole("menuitem", { name: LEARN_FROM_THIS })).toHaveCount(0);
}

test.beforeAll(async ({ stack }) => {
  await pauseQueue(stack);
  await enqueueItem(stack, ITEM);
  await createAgent(stack, AGENT, "Stay idle while Noesis is off.", false);
});

test("Learn from this is hidden on agents and items while Noesis is off", async ({ page, stack }, info) => {
  await pairBoth(page, stack);
  const status = await noesisStatus(stack);
  expect(status.enabled, "Noesis is off by default").toBe(false);
  expect(status.effective.engineering_context, "engineering context is not effective by default").toBe(false);

  await expectNoLearnFromThis(page, stack, "the default config");
  await page.screenshot({ path: info.outputPath("item-off.png") });
});

test("Learn from this stays hidden when engineering context is stored on but Noesis is off", async ({ page, stack }) => {
  await pairBoth(page, stack);
  const stored = await configureNoesis(stack, { engineering_context: true });
  try {
    expect(stored.settings.engineering_context, "engineering context is stored on").toBe(true);
    expect(stored.enabled, "Noesis stays off").toBe(false);
    expect(stored.effective.engineering_context, "a stored engineering context is not effective while Noesis is off").toBe(false);
    await reconnect(page);
    await expectNoLearnFromThis(page, stack, "stored on, Noesis off");
    expect(await noesisFiles(stack), "storing a setting while off made no noesis.db").toEqual([]);
  } finally {
    await configureNoesis(stack, { engineering_context: false });
  }
});
