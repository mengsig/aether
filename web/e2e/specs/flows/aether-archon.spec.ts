// Flow 2, Aether with Archon: one origin serves both. The fleet comes from aetherd, the queue from
// archond through Aether's /archon mount, and an item queued from the page lands in archond.

import { createAgent, enqueueItem, itemTitles, listAgents, pauseQueue } from "../../harness/agents.ts";
import { everyQueueRow, pairBoth } from "../../harness/pair.ts";
import { expect, test } from "../../harness/fixtures.ts";

const AGENT = "Flow fleet agent";
const ITEM = "Queued from the page";
const BRIEF = "Queued by the H4 flow through the mount.";
const HOST_ITEM = "Queued on the host for the flow";

test("the fleet and the queue work through one origin, and a queued item is on both sides", async ({ page, stack }, info) => {
  const sockets: string[] = [];
  page.on("websocket", (socket) => sockets.push(socket.url()));
  // Paused, so the item stays queued on archond rather than starting work on the faux engine.
  await pauseQueue(stack);
  await createAgent(stack, AGENT, "Show on the fleet.", false);
  await pairBoth(page, stack, { aether: "operator", archon: "operator" }, "Flow both");

  const fleet = page.getByRole("region", { name: "Fleet" });
  await expect(fleet.getByRole("link", { name: AGENT })).toBeVisible();
  const agents = await listAgents(stack);
  await expect(fleet.locator(".fleet-count")).toHaveText(`${agents.length} of ${agents.length} agents`);

  await page.goto(`${stack.aether.origin}/#/archon/queue`);
  await expect(page.locator(".queue-pause strong")).toHaveText("Paused");
  await page.getByRole("button", { name: "New item" }).click();
  const dialog = page.getByRole("dialog", { name: "New item" });
  await dialog.getByLabel("Title").fill(ITEM);
  await dialog.getByLabel("Brief").fill(BRIEF);
  await dialog.getByTestId("new-item-pipeline").click();
  await page.getByRole("option", { name: "ask", exact: true }).click();
  await dialog.getByRole("button", { name: "Queue item" }).click();
  await expect(dialog).toHaveCount(0);
  const article = page.getByRole("article", { name: ITEM });
  await expect(article, "queueing opens the new item").toBeVisible();
  await expect(article.getByRole("region", { name: "Brief" })).toContainText(BRIEF);

  await page.goto(`${stack.aether.origin}/#/archon/queue`);
  const queue = page.getByRole("region", { name: "Queue" });
  await expect(queue.getByRole("link", { name: ITEM, exact: true }), "the queue lists the item").toBeVisible();
  await expect.poll(() => itemTitles(stack), { message: "archond holds the item the page queued" }).toContain(ITEM);
  const items = await stack.archon.request({ cmd: "list_item_summaries" });
  const queued = (items.data as unknown as { id: string; title: string; state?: unknown }[]).find((item) => item.title === ITEM);
  expect(queued, "the item is one archond row").toBeDefined();
  const detail = await stack.archon.request({ cmd: "get_item", item: queued?.id });
  expect(JSON.stringify(detail.data), "archond keeps the brief the page typed").toContain(BRIEF);
  await enqueueItem(stack, HOST_ITEM);
  await expect(queue.getByRole("link", { name: HOST_ITEM, exact: true }), "an item queued on the host appears on the page").toBeVisible();
  await expect(await everyQueueRow(page), "the queue lists every item archond holds").toHaveCount((await itemTitles(stack)).length);
  await page.screenshot({ path: info.outputPath("queue-with-page-item.png") });

  const origin = new URL(stack.aether.origin);
  const urls = sockets.map((url) => new URL(url));
  expect(urls.every((url) => url.host === origin.host), `every socket goes to Aether's own origin: ${sockets.join(", ")}`).toBe(true);
  expect(urls.map((url) => url.pathname), "the page reached archond only through the mount").toContain("/archon/ws");
  const remote = await stack.archon.request({ cmd: "remote_status" });
  expect(JSON.stringify(remote.data), "archond paired the page's device").toContain("Flow both");
});
