// Journey 7, learn: Noesis turned on from its settings sheet (main confirms natively), a session
// started on a goal, the Prober answered, and a practice problem shown. Faux engine only.

import { expect, test, type Locator } from "@playwright/test";
import { acceptNativeDialog } from "../../harness/packaged.ts";
import { until } from "../../harness/process.ts";
import { noesisStatus } from "../noesis/noesis.ts";
import { connectionsReady, journeyHooks, type Journey } from "./journey.ts";
import { PRACTICE_STATEMENT, PROBER_QUESTION } from "./scripts.ts";
import { finish, need, open, textField } from "./ui.ts";

const journey = journeyHooks({ archon: false, fauxOnly: "a learning session spawns several teaching agents; the real variant spends only on journey 1's checks" });
const TEACH_MS = 60_000;
const GOAL = "Learn to solve simple linear equations.";

async function setSwitch(sheet: Locator, name: RegExp): Promise<void> {
  const toggle = sheet.getByRole("switch", { name });
  await expect(toggle, `the host may change ${String(name)}`).toBeEnabled();
  await toggle.check();
  await expect(toggle).toBeChecked();
}

/** Noesis on, with the faux model allowed: it is billed per request, not a subscription. */
async function turnNoesisOn(run: Journey): Promise<void> {
  const { window, stack, sway } = run;
  await open(window, "Sessions", "#/learn");
  await (await need(window, "noesis-turn-on", (p) => p.getByTestId("noesis-off").getByRole("button", { name: "Open Noesis settings" }))).click();
  const sheet = window.getByRole("dialog", { name: "Noesis settings" });
  await setSwitch(sheet, /Turn on Noesis/);
  await setSwitch(sheet, /Allow API-billed models/);
  await sheet.getByRole("button", { name: "Save changes" }).click();
  await acceptNativeDialog(sway, "Noesis settings");
  await until(() => noesisStatus(stack), (status) => status.enabled, "Noesis to be on", 30_000);
  await expect(sheet, "the settings sheet closes after saving").toHaveCount(0);
}

test("learn journey: turn Noesis on, start a session, answer the Prober, get a problem", async () => {
  const run = journey();
  const { window } = run;
  await connectionsReady(run, false);
  await turnNoesisOn(run);
  await (await textField(await need(window, "learn-goal", (p) => p.locator("#learn-goal")))).fill(GOAL);
  await (await need(window, "learn-start", (p) => p.getByRole("form", { name: "Start a session" }).getByRole("button", { name: "Start session" }))).click();
  await expect(window.getByText(PROBER_QUESTION).first(), "the Prober asks").toBeVisible({ timeout: TEACH_MS });
  await (await textField(await need(window, "learn-reply", (p) => p.getByRole("textbox", { name: "Your answer" })))).fill("two times x");
  await (await need(window, "learn-send", (p) => p.getByRole("button", { name: /^(Answer|Send)$/ }))).click();
  await expect(window.getByText(PRACTICE_STATEMENT).first(), "a practice problem appears").toBeVisible({ timeout: TEACH_MS });
  finish();
});
