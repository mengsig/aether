// Journey 8, drafts: what is typed into the start sheet's objective survives leaving Activity
// and coming back.

import { expect, test } from "@playwright/test";
import { connectionsReady, journeyHooks } from "./journey.ts";
import { shootWindow } from "./capture.ts";
import { openStart, startSheet } from "./steps.ts";
import { need, open, textField } from "./ui.ts";

const journey = journeyHooks({ archon: false });
const DRAFT = "Draft objective: tidy the release checklist.";

test("drafts journey: a typed objective is still there after navigating away", async () => {
  const { window } = journey();
  await connectionsReady(journey(), false);
  await openStart(window);
  await (await textField(await need(startSheet(window), "start-objective"))).fill(DRAFT);
  await startSheet(window).getByRole("button", { name: "Close Start an agent", exact: true }).click();
  await expect(startSheet(window), "the start sheet closes").toHaveCount(0);
  await open(window, "Fleet", "#/fleet");
  await expect(window.getByRole("region", { name: "Fleet" }), "the window left Activity").toBeVisible();
  await openStart(window);
  await expect(await textField(await need(startSheet(window), "start-objective")), "the draft survived").toHaveValue(DRAFT);
  await shootWindow(window, "start-agent", "draft-restored");
});
