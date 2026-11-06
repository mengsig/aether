// The desktop app's smallest window (720x480, a 720x451 page under its menu): every screen a
// user works in reflows into the width, so the page itself never scrolls sideways, and the
// controls form stacks instead of overlapping.

import { createAgent } from "../../harness/agents.ts";
import { pairBoth } from "../../harness/pair.ts";
import { expect, test, type Page } from "../../harness/fixtures.ts";
import { openAgent } from "../noesis/noesis.ts";

const MIN_WINDOW = { width: 720, height: 451 };

/** The page's scroll width against its viewport, and the shell main's, after the screen settled. */
async function widths(page: Page): Promise<{ readonly page: number; readonly viewport: number; readonly main: number }> {
  await expect(page.locator(".ui-screen-skeleton")).toHaveCount(0);
  return page.evaluate(() => ({
    page: document.documentElement.scrollWidth,
    viewport: window.innerWidth,
    main: document.querySelector(".ui-shell-main")?.scrollWidth ?? 0,
  }));
}

test("at the minimum window size no screen scrolls sideways and the controls form stacks", async ({ page, stack }, info) => {
  await pairBoth(page, stack);
  const agentId = await createAgent(stack, "Narrow worker", "Stay idle for the narrow-window smoke.", false);
  const conn = encodeURIComponent(await openAgent(page, stack, "Narrow worker"));
  await page.setViewportSize(MIN_WINDOW);
  const controls = `#/controls/${conn}/${agentId}`;
  const screens = [
    ["settings", "#/settings/providers"],
    ["activity", "#/activity"],
    ["controls", controls],
    ["chat", `#/agents/${conn}/${agentId}`],
    ["learn", "#/learn"],
  ] as const;
  for (const [name, hash] of screens) {
    await page.goto(`${stack.aether.origin}/${hash}`);
    if (name === "controls") await openModelTab(page);
    await page.waitForTimeout(300);
    const measured = await widths(page);
    expect(measured.page, `${name} fits the window: ${JSON.stringify(measured)}`).toBeLessThanOrEqual(measured.viewport);
    expect(measured.main, `${name}'s main does not scroll sideways: ${JSON.stringify(measured)}`).toBeLessThanOrEqual(measured.viewport);
    await page.screenshot({ path: info.outputPath(`min-window-${name}.png`), fullPage: true });
  }
  await page.goto(`${stack.aether.origin}/${controls}`);
  await openModelTab(page);
  await expectStacked(page);
});

async function openModelTab(page: Page): Promise<void> {
  await page.getByRole("tab", { name: "Model & limits" }).click();
  await expect(page.getByTestId("configure-model")).toBeVisible();
}

/** The effort choice sits below the model, and the apply button below the effort, never beside them. */
async function expectStacked(page: Page): Promise<void> {
  const model = await page.getByTestId("configure-model").boundingBox();
  const effort = await page.getByRole("radiogroup", { name: "Effort" }).boundingBox();
  const apply = await page.getByRole("button", { name: "Apply model" }).boundingBox();
  expect(model !== null && effort !== null && apply !== null, "the model form's controls are laid out").toBe(true);
  if (model === null || effort === null || apply === null) return;
  expect(effort.y, "the effort row is below the model").toBeGreaterThanOrEqual(model.y + model.height);
  expect(apply.y, "the apply button is below the effort row").toBeGreaterThanOrEqual(effort.y + effort.height);
}
