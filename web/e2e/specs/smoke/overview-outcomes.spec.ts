import { enqueueItem, pauseQueue } from "../../harness/agents.ts";
import { expect, test } from "../../harness/fixtures.ts";
import { pairBoth } from "../../harness/pair.ts";

const TITLE = "A very long outcome title that must leave room for its status and its completion time on the overview dashboard";

test("long recent outcomes keep their title, status and time separate", async ({ page, stack }, testInfo) => {
  await pauseQueue(stack);
  const item = await enqueueItem(stack, TITLE);
  const cancelled = await stack.archon.request({ cmd: "cancel_item", item, reason: "layout regression" });
  expect(cancelled.reply).not.toBe("error");
  await pairBoth(page, stack, { aether: "operator", archon: "operator" }, "Overview layout");
  await page.goto(`${stack.aether.origin}/#/`);

  const row = page.getByRole("list", { name: "Recent outcomes" }).getByRole("listitem").filter({ hasText: TITLE });
  const title = row.getByRole("link", { name: TITLE });
  const status = row.locator(".ui-pill");
  const time = row.locator("time");
  await expect(title).toBeVisible();
  await expect(status).toHaveText("Cancelled");
  await expect(time).toBeVisible();

  for (const width of [1280, 1440, 1920]) {
    for (const scheme of ["light", "dark"] as const) {
      await page.setViewportSize({ width, height: 1000 });
      await page.emulateMedia({ colorScheme: scheme, reducedMotion: "reduce" });
      const boxes = await Promise.all([row.boundingBox(), title.boundingBox(), status.boundingBox(), time.boundingBox()]);
      const [rowBox, titleBox, statusBox, timeBox] = boxes;
      expect(rowBox).not.toBeNull();
      expect(titleBox).not.toBeNull();
      expect(statusBox).not.toBeNull();
      expect(timeBox).not.toBeNull();
      if (!rowBox || !titleBox || !statusBox || !timeBox) throw new Error("Outcome row has no layout box");
      expect(titleBox.x + titleBox.width).toBeLessThanOrEqual(statusBox.x);
      expect(statusBox.x + statusBox.width).toBeLessThanOrEqual(timeBox.x);
      expect(timeBox.x + timeBox.width).toBeLessThanOrEqual(rowBox.x + rowBox.width + 0.5);
      expect(await title.evaluate((element) => element.scrollWidth > element.clientWidth)).toBe(true);
      expect(await status.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
      await testInfo.attach(`outcomes-${width}-${scheme}`, { body: await row.screenshot(), contentType: "image/png" });
    }
  }
});
