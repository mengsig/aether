// Drafts survive navigation: what a person types into a form stays while they look elsewhere, comes
// back on reload of the same tab, and is cleared by a successful submit.

import { pauseQueue } from "../../harness/agents.ts";
import { pairBoth } from "../../harness/pair.ts";
import { expect, test } from "../../harness/fixtures.ts";

const TITLE = "Draft kept while browsing";
const BRIEF = "Typed, left, and found again.";
const OBJECTIVE = "An objective typed before leaving the screen.";

test("a new-item draft and a start-agent draft outlive navigation and reload, and a submit clears its draft", async ({ page, stack }) => {
  await pauseQueue(stack);
  await pairBoth(page, stack, { aether: "operator", archon: "operator" }, "Drafts");

  await page.goto(`${stack.aether.origin}/#/archon/queue`);
  await page.getByRole("button", { name: "New item" }).click();
  const dialog = page.getByRole("dialog", { name: "New item" });
  await dialog.getByTestId("new-item-title").fill(TITLE);
  await dialog.getByTestId("new-item-brief").fill(BRIEF);
  await dialog.getByRole("button", { name: "Cancel" }).click();

  await page.goto(`${stack.aether.origin}/#/activity`);
  await page.getByRole("button", { name: "Start an agent" }).first().click();
  await page.getByTestId("start-objective").fill(OBJECTIVE);
  await page.goto(`${stack.aether.origin}/#/fleet`);
  await expect(page.getByRole("region", { name: "Fleet" })).toBeVisible();

  await page.goto(`${stack.aether.origin}/#/activity`);
  await page.getByRole("button", { name: "Start an agent" }).first().click();
  await expect(page.getByTestId("start-objective"), "the objective survives leaving the screen").toHaveValue(OBJECTIVE);

  await page.reload();
  await page.goto(`${stack.aether.origin}/#/archon/queue`);
  await page.getByRole("button", { name: "New item" }).click();
  await expect(dialog.getByTestId("new-item-title"), "the title survives a reload of the tab").toHaveValue(TITLE);
  await expect(dialog.getByTestId("new-item-brief")).toHaveValue(BRIEF);

  await dialog.getByTestId("new-item-pipeline").click();
  await page.getByRole("option", { name: "ask", exact: true }).click();
  await dialog.getByTestId("new-item-submit").click();
  await expect(dialog).toHaveCount(0);
  await expect(page.getByRole("article", { name: TITLE })).toBeVisible();

  await page.goto(`${stack.aether.origin}/#/archon/queue`);
  await page.getByRole("button", { name: "New item" }).click();
  await expect(dialog.getByTestId("new-item-title"), "a successful submit clears the draft").toHaveValue("");
  await expect(page.evaluate(() => sessionStorage.getItem("aether.drafts.v1") ?? ""), "the mirror no longer holds the item").resolves.not.toContain(TITLE);
});
