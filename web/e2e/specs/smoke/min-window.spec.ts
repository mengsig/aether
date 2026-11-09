// The desktop app's smallest window (720x480, a 720x451 page under its menu): every screen a
// user works in reflows into the width, so the page itself never scrolls sideways, and the
// controls form stacks instead of overlapping.

import { createAgent } from "../../harness/agents.ts";
import { pairBoth } from "../../harness/pair.ts";
import { expect, test, type Page } from "../../harness/fixtures.ts";
import { openAgent } from "../noesis/noesis.ts";

const MIN_WINDOW = { width: 720, height: 451 };

/** The page's scroll width against its viewport, and the shell main's, once `ready` shows. */
async function widths(page: Page, ready: string): Promise<{ readonly page: number; readonly viewport: number; readonly main: number }> {
  await expect(page.locator(ready).first(), `the screen is populated: ${ready}`).toBeVisible();
  await expect(page.locator(".ui-shell-main"), "the shell's main is there to measure").toHaveCount(1);
  return page.evaluate(() => ({
    page: document.documentElement.scrollWidth,
    viewport: window.innerWidth,
    main: document.querySelector(".ui-shell-main")?.scrollWidth ?? Number.NaN,
  }));
}

test("at the minimum window size no screen scrolls sideways and the controls form stacks", async ({ page, stack }, info) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await pairBoth(page, stack);
  const agentId = await createAgent(stack, "Narrow worker", "Stay idle for the narrow-window smoke.", false);
  const conn = encodeURIComponent(await openAgent(page, stack, "Narrow worker"));
  await page.setViewportSize(MIN_WINDOW);
  const controls = `#/controls/${conn}/${agentId}`;
  // Each screen with what shows once its data is there, not its loading skeleton.
  const screens = [
    ["settings", "#/settings/providers", '[role="tabpanel"]'],
    ["activity", "#/activity", "article[data-kind]"],
    ["controls", controls, '[data-testid="configure-model"]'],
    ["chat", `#/agents/${conn}/${agentId}`, '[data-testid="composer-input"]'],
    ["learn", "#/learn", '[data-testid="noesis-turn-on"]'],
  ] as const;
  for (const [name, hash, ready] of screens) {
    await page.goto(`${stack.aether.origin}/${hash}`);
    if (name === "controls") await openModelTab(page);
    const measured = await widths(page, ready);
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
  await expect(page.getByRole("radiogroup", { name: "Effort" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Apply model", exact: true })).toBeVisible();
  const bounds = await page.evaluate(() => {
    const model = document.querySelector('[data-testid="configure-model"]');
    const effort = document.querySelector('[role="radiogroup"][aria-label="Effort"]');
    const apply = Array.from(document.querySelectorAll("button")).find((button) => button.textContent?.trim() === "Apply model");
    if (!model || !effort || !apply) return null;
    const modelBox = model.getBoundingClientRect();
    const effortBox = effort.getBoundingClientRect();
    const applyBox = apply.getBoundingClientRect();
    return { modelBottom: modelBox.bottom, effortTop: effortBox.top, effortBottom: effortBox.bottom, applyTop: applyBox.top };
  });
  expect(bounds, "the model form's controls are laid out").not.toBeNull();
  if (bounds === null) return;
  expect(bounds.effortTop, "the effort row is below the model").toBeGreaterThanOrEqual(bounds.modelBottom);
  expect(bounds.applyTop, "the apply button is below the effort row").toBeGreaterThanOrEqual(bounds.effortBottom);
}
