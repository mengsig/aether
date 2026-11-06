// Journey 7, learn: Noesis turned on from its settings sheet (main confirms natively), a session
// started on a goal, the Prober answered, and a practice problem shown. Faux engine only.

import { expect, test, type Locator, type Page } from "@playwright/test";
import { acceptNativeDialog } from "../../harness/packaged.ts";
import { until } from "../../harness/process.ts";
import { noesisStatus } from "../noesis/noesis.ts";
import { connectionsReady, journeyHooks, type Journey } from "./journey.ts";
import { PRACTICE_STATEMENT, PROBER_QUESTION } from "./scripts.ts";
import { need, open, textField } from "./ui.ts";

const journey = journeyHooks({ archon: false, fauxOnly: "a learning session spawns several teaching agents; the real variant spends only on journey 1's checks" });
const TEACH_MS = 60_000;
const GOAL = "Learn to solve simple linear equations.";

async function switchOn(sheet: Locator, id: string): Promise<void> {
  const toggle = await need(sheet, id);
  await expect(toggle, `the host may change [data-testid=${id}]`).toBeEnabled();
  await toggle.check();
  await expect(toggle).toBeChecked();
}

/** Noesis on from its guided turn-on, with the faux model allowed: it is billed per request, not a subscription. */
async function turnNoesisOn(run: Journey): Promise<void> {
  const { window, stack, sway } = run;
  await open(window, "Sessions", "#/learn");
  await (await need(await need(window, "noesis-turn-on"), "noesis-turn-on-action")).click();
  const sheet = window.getByRole("dialog", { name: "Noesis settings" });
  await expect(sheet, "the turn-on opens Noesis settings").toBeVisible();
  await expect(await need(sheet, "noesis-enable"), "the turn-on already chose Noesis on").toBeChecked();
  await switchOn(sheet, "noesis-allow-api");
  const save = await need(sheet, "noesis-save");
  await expect(save, "the settings are ready to save").toBeEnabled();
  await save.click();
  await acceptNativeDialog(sway, "Noesis settings");
  await until(() => noesisStatus(stack), (status) => status.enabled, "Noesis to be on", 30_000);
  await expect(sheet, "the settings sheet closes after saving").toHaveCount(0);
}

/** Starts a session on GOAL from the Sessions screen; it lands on the session. */
async function startSession(window: Page): Promise<Locator> {
  await (await textField(await need(window, "learn-goal"))).fill(GOAL);
  await expect(await need(window, "learn-mode"), "the session kind is offered").toBeVisible();
  const start = await need(window, "learn-start");
  await expect(start, "the session is ready to start").toBeEnabled();
  await start.click();
  await expect(window, "starting lands on the session").toHaveURL(/#\/learn\/sessions\/[^/]+$/);
  return need(window, "learn-transcript", TEACH_MS);
}

test("learn journey: turn Noesis on, start a session, answer the Prober, get a problem", async () => {
  const run = journey();
  const { window } = run;
  await connectionsReady(run, false);
  await turnNoesisOn(run);
  const transcript = await startSession(window);
  await expect(transcript.getByText(PROBER_QUESTION).first(), "the Prober asks").toBeVisible({ timeout: TEACH_MS });
  await expect(await need(window, "learn-reply"), "an open question is answered in its card, not the composer").toBeDisabled();
  const card = transcript.locator("form").filter({ has: window.getByTestId("learn-answer") }).last();
  await (await textField(await need(card, "learn-answer"))).fill("two times x");
  await card.getByRole("button", { name: "Answer", exact: true }).click();
  await expect(transcript.getByText(PRACTICE_STATEMENT).first(), "a practice problem appears").toBeVisible({ timeout: TEACH_MS });
});
