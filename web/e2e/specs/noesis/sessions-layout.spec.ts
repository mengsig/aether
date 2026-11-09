import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import path from "node:path";
import { expect, test, type Page } from "../../harness/fixtures.ts";
import { pairBoth } from "../../harness/pair.ts";
import type { Stack } from "../../harness/stack.ts";
import { configureNoesis } from "./noesis.ts";

const GOALS = [
  `Understand ${"interoperability".repeat(35)} and explain it clearly`,
  "Learn how a long learning goal with many ordinary words wraps within the sessions list and remains usable as a recent goal",
  "Practice reasoning about distributed systems and failure recovery",
] as const;
const IDS = GOALS.map((_, index) => `ns_layout_regression_${index}`);

function seedSessions(stack: Stack, remove: boolean): void {
  const db = new DatabaseSync(path.join(stack.layout.aether.data, "noesis.db"));
  try {
    const deleteSession = db.prepare("DELETE FROM sessions WHERE id = ?");
    const insertSession = db.prepare("INSERT INTO sessions (id, goal, mode, phase, agent, created_at, updated_at) VALUES (?, ?, 'teach', 'probe', ?, ?, ?)");
    for (const [index, id] of IDS.entries()) {
      expect(deleteSession.run(id).changes).toBeLessThanOrEqual(1);
      if (remove) continue;
      const goal = GOALS[index];
      assert.ok(goal, "each seeded session has a goal");
      const at = `2026-11-0${index + 1}T12:00:00Z`;
      expect(insertSession.run(id, goal, `agt_0000000000000000000000000${index}`, at, at).changes).toBe(1);
    }
  } finally {
    db.close();
  }
}

async function expectContained(page: Page, child: string, parent: string): Promise<void> {
  const bounds = await page.locator(child).evaluateAll((elements, parentSelector) => elements.map((element) => {
    const outer = element.closest(parentSelector)?.getBoundingClientRect();
    const inner = element.getBoundingClientRect();
    return outer === undefined ? null : { left: inner.left - outer.left, right: outer.right - inner.right };
  }), parent);
  expect(bounds.length).toBeGreaterThan(0);
  for (const bound of bounds) {
    assert.ok(bound, `${child} has a ${parent} ancestor`);
    expect(bound.left).toBeGreaterThanOrEqual(-1);
    expect(bound.right).toBeGreaterThanOrEqual(-1);
  }
}

test.beforeAll(async ({ stack }) => {
  await configureNoesis(stack, { enabled: true });
  seedSessions(stack, false);
});

test.afterAll(async ({ stack }) => {
  seedSessions(stack, true);
  await configureNoesis(stack, { enabled: false });
});

test("long goals stay in their columns and the practice picker remains clickable", async ({ page, stack }, info) => {
  await pairBoth(page, stack);
  await page.goto(`${stack.aether.origin}/#/learn`);
  await expect(page.locator(".learn-card-goal")).toHaveCount(GOALS.length);
  await expect(page.locator(".learn-chip")).toHaveCount(GOALS.length);

  for (const width of [1280, 1440, 1920]) {
    await page.setViewportSize({ width, height: 1000 });
    const start = await page.locator(".learn-start").boundingBox();
    const list = await page.locator(".learn-list").boundingBox();
    assert.ok(start, "the start column is visible");
    assert.ok(list, "the sessions column is visible");
    expect(start.x + start.width).toBeLessThanOrEqual(list.x);
    await expectContained(page, ".learn-card-goal", ".learn-card");
    await expectContained(page, ".learn-chip", ".learn-new");
    await expectContained(page, '[data-testid="learn-mode"]', ".learn-new");
    const practice = page.getByRole("radio", { name: /Practise what I know/ });
    await practice.click();
    await expect(practice).toHaveAttribute("aria-checked", "true");
    for (const theme of ["light", "dark"] as const) {
      await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
      await expect(page.locator("html")).toHaveAttribute("data-theme", theme);
      await page.screenshot({ path: info.outputPath(`learn-sessions-${width}-${theme}.png`) });
    }
  }
  await page.locator(".learn-chip").filter({ hasText: GOALS[0] }).click();
  await expect(page.getByTestId("learn-goal")).toHaveValue(GOALS[0]);
});
