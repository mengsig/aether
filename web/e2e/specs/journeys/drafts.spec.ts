// Journey 8, drafts: what is typed into the start sheet's objective survives leaving Activity
// and coming back. The draft store is new, so this journey has no fallback for today's sheet.

import { expect, test } from "@playwright/test";
import { connectionsReady, journeyHooks } from "./journey.ts";
import { openStart } from "./steps.ts";
import { control, finish, need, open, textField } from "./ui.ts";

const journey = journeyHooks({ archon: false });
const DRAFT = "Draft objective: tidy the release checklist.";

test("drafts journey: a typed objective is still there after navigating away", async () => {
  const { window } = journey();
  await connectionsReady(journey(), false);
  await openStart(window);
  await (await textField(await need(window, "start-objective"))).fill(DRAFT);
  await open(window, "Fleet", "#/fleet");
  await expect(window.getByRole("region", { name: "Fleet" }), "the window left Activity").toBeVisible();
  await open(window, "Activity", "#/activity");
  if ((await control(window, "start-objective", undefined, 3_000)) === null) {
    await (await need(window, "start-agent", (p) => p.getByRole("button", { name: "Start an agent" }))).click();
  }
  await expect(await textField(await need(window, "start-objective")), "the draft survived").toHaveValue(DRAFT);
  finish();
});
