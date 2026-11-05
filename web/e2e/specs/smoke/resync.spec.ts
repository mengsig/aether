// Resync after a daemon restart: the page sees the connection drop, reconnects by itself, and
// shows what changed while it was away, with no reload and nothing stale left on screen.

import { createAgent, enqueueItem, itemTitles, listAgents, pauseQueue, renameAgent, retitleItem } from "../../harness/agents.ts";
import { connection, pairBoth } from "../../harness/pair.ts";
import { expect, test, type Page } from "../../harness/fixtures.ts";

async function markPage(page: Page): Promise<void> {
  await page.evaluate(() => { (window as { e2eNoReload?: boolean }).e2eNoReload = true; });
}

async function notReloaded(page: Page): Promise<boolean> {
  return page.evaluate(() => (window as { e2eNoReload?: boolean }).e2eNoReload === true);
}

test("the fleet recovers from an aetherd restart without a reload", async ({ page, stack }, info) => {
  await pairBoth(page, stack);
  const before = await createAgent(stack, "Before restart", "Wait for the restart.", false);
  const fleet = page.getByRole("region", { name: "Fleet" });
  await expect(fleet.getByRole("link", { name: "Before restart" })).toBeVisible();
  await markPage(page);

  await stack.aether.stop();
  await expect(connection(page, "aether")).not.toHaveAttribute("data-state", "ready");
  await stack.aether.start();
  await renameAgent(stack, before, "Renamed after restart");
  await createAgent(stack, "Created after restart", "Arrive after the restart.");

  await expect(connection(page, "aether")).toHaveAttribute("data-state", "ready", { timeout: 30_000 });
  await expect(fleet.getByRole("link", { name: "Renamed after restart" })).toBeVisible();
  await expect(fleet.getByRole("link", { name: "Created after restart" })).toBeVisible();
  await expect(fleet.getByRole("link", { name: "Before restart" })).toHaveCount(0);
  const agents = await listAgents(stack);
  await expect(fleet.locator(".fleet-count")).toHaveText(`${agents.length} of ${agents.length} agents`);
  expect(await notReloaded(page), "the page was not reloaded").toBe(true);
  await page.screenshot({ path: info.outputPath("fleet-after-aetherd-restart.png") });
});

test("the queue recovers from an archond restart without a reload", async ({ page, stack }, info) => {
  await pauseQueue(stack);
  const before = await enqueueItem(stack, "Before archond restart");
  await pairBoth(page, stack);
  await page.goto(`${stack.aether.origin}/#/archon/queue`);
  const queue = page.getByRole("region", { name: "Queue" });
  await expect(queue.getByRole("link", { name: "Before archond restart" })).toBeVisible();
  await markPage(page);

  await stack.archon.stop();
  await expect(connection(page, "archon")).not.toHaveAttribute("data-state", "ready");
  await expect(connection(page, "aether")).toHaveAttribute("data-state", "ready");
  await stack.archon.start();
  await retitleItem(stack, before, "Retitled after archond restart");
  await enqueueItem(stack, "Queued after archond restart");

  await expect(connection(page, "archon")).toHaveAttribute("data-state", "ready", { timeout: 30_000 });
  await expect(queue.getByRole("link", { name: "Retitled after archond restart" })).toBeVisible();
  await expect(queue.getByRole("link", { name: "Queued after archond restart" })).toBeVisible();
  await expect(queue.getByRole("link", { name: "Before archond restart" })).toHaveCount(0);
  await expect(queue.locator(".queue-row")).toHaveCount((await itemTitles(stack)).length);
  expect(await notReloaded(page), "the page was not reloaded").toBe(true);
  await page.screenshot({ path: info.outputPath("queue-after-archond-restart.png") });
});
