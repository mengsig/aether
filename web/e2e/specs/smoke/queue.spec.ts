// The Archon queue through Aether's /archon mount: an operator action from the page reaches
// archond, and an item queued on the host appears without a reload.

import { enqueueItem, itemTitles, queuePaused } from "../../harness/agents.ts";
import { pairBoth } from "../../harness/pair.ts";
import { expect, test } from "../../harness/fixtures.ts";

test("the queue operates archond through the mount and follows its items live", async ({ page, stack }, info) => {
  await pairBoth(page, stack);
  await page.goto(`${stack.aether.origin}/#/archon/queue`);
  const queue = page.getByRole("region", { name: "Queue" });
  const state = page.locator(".queue-pause strong");
  if (!(await queuePaused(stack))) {
    await expect(state).toHaveText("Running");
    await page.getByRole("button", { name: "Pause new work" }).click();
  }
  await expect(state).toHaveText("Paused");
  await expect.poll(() => queuePaused(stack), { message: "the page's pause reached archond" }).toBe(true);

  await enqueueItem(stack, "Queued on the host");
  await expect(queue.getByRole("link", { name: "Queued on the host" })).toBeVisible();
  const titles = await itemTitles(stack);
  await expect(queue.locator(".queue-row")).toHaveCount(titles.length);
  await page.screenshot({ path: info.outputPath("queue.png") });
});
