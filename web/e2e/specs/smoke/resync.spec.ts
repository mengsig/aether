// Resync after a daemon restart: the page sees the connection drop, reconnects by itself, and
// shows what changed while it was away, with no reload and nothing stale left on screen.

import { createAgent, enqueueItem, itemTitles, listAgents, pauseQueue, renameAgent, retitleItem } from "../../harness/agents.ts";
import { connection, everyQueueRow, fleetLink, pairBoth } from "../../harness/pair.ts";
import { expect, test, type Page } from "../../harness/fixtures.ts";
import { openAgent } from "../noesis/noesis.ts";
import { DEFAULT_REPLY } from "../../harness/engine.ts";

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
  await fleetLink(page, "Before restart");
  await markPage(page);

  await stack.aether.stop();
  await expect(connection(page, "aether")).not.toHaveAttribute("data-state", "ready");
  await stack.aether.start();
  await renameAgent(stack, before, "Renamed after restart");
  await createAgent(stack, "Created after restart", "Arrive after the restart.");

  await expect(connection(page, "aether")).toHaveAttribute("data-state", "ready", { timeout: 30_000 });
  await fleetLink(page, "Renamed after restart");
  await fleetLink(page, "Created after restart");
  await expect(fleet.getByRole("link", { name: "Before restart" })).toHaveCount(0);
  const agents = await listAgents(stack);
  await expect(fleet.locator(".fleet-count")).toHaveText(`${agents.length} of ${agents.length} agents`);
  expect(await notReloaded(page), "the page was not reloaded").toBe(true);
  await page.screenshot({ path: info.outputPath("fleet-after-aetherd-restart.png") });
});

test("usage and paired devices remain visible in both themes", async ({ page, stack }, info) => {
  test.setTimeout(150_000);
  await pairBoth(page, stack);
  await createAgent(stack, "Usage example", "Answer briefly.");
  await openAgent(page, stack, "Usage example");
  await expect(page.locator(".conv-meta").first()).toBeVisible({ timeout: 60_000 });
  for (const scheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    await page.screenshot({ path: info.outputPath(`chat-usage-${scheme}.png`) });
  }
  await page.goto(`${stack.aether.origin}/#/`);
  await expect(page.locator('[data-area="spend"] .ui-chart-tile-value')).toBeVisible();
  for (const scheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    await page.screenshot({ path: info.outputPath(`overview-spend-${scheme}.png`) });
  }
  await page.goto(`${stack.aether.origin}/#/settings/devices`);
  await expect(page.getByText("No devices paired")).toHaveCount(0);
  await expect(page.locator(".st-item").first()).toBeVisible();
  for (const scheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    await page.screenshot({ path: info.outputPath(`devices-${scheme}.png`) });
  }
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
  await expect(queue.getByRole("status").filter({ hasText: "Queue data is stale" })).toBeVisible();
  for (const scheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    await page.screenshot({ path: info.outputPath(`queue-disconnected-${scheme}.png`) });
  }
  await stack.archon.start();
  await retitleItem(stack, before, "Retitled after archond restart");
  await enqueueItem(stack, "Queued after archond restart");

  await expect(connection(page, "archon")).toHaveAttribute("data-state", "ready", { timeout: 30_000 });
  await expect(queue.getByText("Queue data is stale", { exact: false })).toHaveCount(0);
  await expect(queue.getByRole("link", { name: "Retitled after archond restart" })).toBeVisible();
  await expect(queue.getByRole("link", { name: "Queued after archond restart" })).toBeVisible();
  await expect(queue.getByRole("link", { name: "Before archond restart" })).toHaveCount(0);
  await expect(await everyQueueRow(page)).toHaveCount((await itemTitles(stack)).length);
  expect(await notReloaded(page), "the page was not reloaded").toBe(true);
  await page.screenshot({ path: info.outputPath("queue-after-archond-restart.png") });
});

test("an interrupted tool is no longer shown as running after reconnect", async ({ page, stack }, info) => {
  test.setTimeout(150_000);
  stack.engine.setScript(() => ({ toolCall: { name: "bash", arguments: { command: "sleep 150" }, text: "Waiting for the tool." } }));
  try {
    await pairBoth(page, stack);
    await createAgent(stack, "Interrupted tool", "Run the waiting tool.");
    await openAgent(page, stack, "Interrupted tool");
    const tool = page.locator('.conv-tool[data-state="running"]').first();
    await expect(tool).toBeVisible({ timeout: 60_000 });
    await stack.aether.stop();
    await stack.aether.start();
    await expect(connection(page, "aether")).toHaveAttribute("data-state", "ready", { timeout: 30_000 });
    const interrupted = page.locator('.conv-tool[data-state="interrupted"]').first();
    await expect(interrupted).toBeVisible({ timeout: 30_000 });
    for (const scheme of ["light", "dark"] as const) {
      await page.emulateMedia({ colorScheme: scheme });
      await page.screenshot({ path: info.outputPath(`tool-interrupted-${scheme}.png`) });
    }
  } finally {
    stack.engine.setScript(() => DEFAULT_REPLY);
  }
});
