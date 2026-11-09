import assert from "node:assert/strict";
import { createAgent } from "../../harness/agents.ts";
import { pairBoth } from "../../harness/pair.ts";
import { expect, test, type Page } from "../../harness/fixtures.ts";
import { openAgent } from "../noesis/noesis.ts";

async function expectChatLayout(page: Page): Promise<void> {
  const screen = await page.locator(".agent").boundingBox();
  const main = await page.locator(".ui-shell-main").boundingBox();
  const toolbar = await page.locator(".conv-toolbar").boundingBox();
  const thread = await page.locator(".conv-list").boundingBox();
  const composer = await page.locator(".cmp").boundingBox();
  const message = await page.locator(".conv-msg").first().boundingBox();
  assert.ok(screen && main && toolbar && thread && composer && message, "chat and its messages are laid out");
  expect(screen.width).toBeGreaterThan(main.width - 100);
  for (const box of [toolbar, thread, composer]) {
    expect(Math.abs(box.x - screen.x)).toBeLessThanOrEqual(2);
    expect(Math.abs(box.width - screen.width)).toBeLessThanOrEqual(2);
  }
  expect(toolbar.y + toolbar.height).toBeLessThanOrEqual(thread.y + 1);
  expect(thread.y + thread.height).toBeLessThanOrEqual(composer.y + 1);
  expect(message.width).toBeGreaterThan(thread.width - 40);
  const widths = await page.locator(".conv-list").evaluate((list) => ({
    page: document.documentElement.scrollWidth, viewport: innerWidth, thread: list.scrollWidth, client: list.clientWidth,
  }));
  expect(widths.page).toBeLessThanOrEqual(widths.viewport);
  expect(widths.thread).toBeLessThanOrEqual(widths.client);
}

test("chat fills the available width without overlap on large displays", async ({ page, stack }) => {
  await pairBoth(page, stack);
  stack.engine.setScript(() => ({ toolCall: { name: "bash", arguments: { command: "sleep 150" }, text: "Checking the layout." } }));
  await createAgent(stack, "Wide worker", "Check the wide-window layout.");
  await openAgent(page, stack, "Wide worker");
  await expect(page.getByTestId("composer-input")).toBeVisible();
  await expect(page.locator(".conv-toolbar")).toBeVisible();
  for (const size of [{ width: 1920, height: 1080 }, { width: 2560, height: 1440 }, { width: 3440, height: 1440 }]) {
    await page.setViewportSize(size);
    for (const theme of ["light", "dark"] as const) {
      await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
      await expect(page.locator("html")).toHaveAttribute("data-theme", theme);
      await expectChatLayout(page);
    }
  }
});
