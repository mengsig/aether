// Finding the journeys' controls: the new UI's test id first, else today's equivalent control by
// role or label. A step whose control is in neither is recorded as missing; a journey with any
// missing step ends skipped, naming them, so the lead can flip it on once the new UI lands.

import assert from "node:assert/strict";
import { errors, expect, test, type Locator, type Page } from "@playwright/test";

export const WAITS = "waits for the new UI";
const FIND_MS = 8_000;

export type Fallback = (page: Page) => Locator;

/** Which locator a control was found by; recorded on the test for the report. */
export function note(type: "fallback" | "missing", description: string): void {
  test.info().annotations.push({ type, description });
}

/** The control with test id `id`, else `fallback`'s; null when neither shows within the bound. */
export async function control(page: Page, id: string, fallback?: Fallback, timeoutMs = FIND_MS): Promise<Locator | null> {
  assert.match(id, /^[a-z][a-z0-9-]*$/, `a test id is kebab-case: ${id}`);
  const byId = page.getByTestId(id).first();
  const alternative = fallback?.(page).first();
  const either = alternative === undefined ? byId : byId.or(alternative).first();
  try {
    await either.waitFor({ state: "visible", timeout: timeoutMs });
  } catch (error) {
    // Absent is an expected answer here: the caller skips or records the step.
    if (error instanceof errors.TimeoutError) return null;
    throw error;
  }
  if (await byId.count() > 0) return byId;
  assert.ok(alternative !== undefined, `[data-testid=${id}] showed, then vanished`);
  note("fallback", `${id}: today's control by role or label`);
  return alternative;
}

/** A control the rest of the journey depends on: missing, the test skips here, naming it. */
export async function need(page: Page, id: string, fallback?: Fallback, timeoutMs = FIND_MS): Promise<Locator> {
  const found = await control(page, id, fallback, timeoutMs);
  if (found === null) {
    note("missing", id);
    test.skip(true, `${WAITS}: [data-testid=${id}] is missing`);
  }
  assert.ok(found !== null, "test.skip ended the test");
  return found;
}

/** A step later steps do not depend on: run when its control shows, recorded as missing otherwise. */
export async function optional(page: Page, id: string, fallback: Fallback | undefined, body: (found: Locator) => Promise<void>): Promise<boolean> {
  const found = await control(page, id, fallback);
  if (found === null) {
    note("missing", id);
    return false;
  }
  await body(found);
  return true;
}

/** Ends a journey: skipped, naming every missing control, when any step had none. */
export function finish(): void {
  const missing = test.info().annotations.filter((annotation) => annotation.type === "missing").map((annotation) => annotation.description ?? "");
  const named = missing.map((what) => (/^[a-z][a-z0-9-]*$/.test(what) ? `[data-testid=${what}]` : what));
  test.skip(missing.length > 0, `${WAITS}: missing ${named.join(", ")}; every other step passed`);
}

/** Whether `found` shows within the bound; recorded as missing (`what`) when it does not. */
export async function shows(found: Locator, what: string, timeoutMs = FIND_MS): Promise<boolean> {
  assert.ok(what.trim() !== "", "a missing view is named");
  try {
    await found.first().waitFor({ state: "visible", timeout: timeoutMs });
    return true;
  } catch (error) {
    if (!(error instanceof errors.TimeoutError)) throw error;
    note("missing", what);
    return false;
  }
}

/** Whether `found` is the new UI's control and not today's fallback. */
export async function isTestId(found: Locator, id: string): Promise<boolean> {
  return (await found.getAttribute("data-testid")) === id;
}

/** The text field itself, or the one a test-id wrapper holds. */
export async function textField(found: Locator): Promise<Locator> {
  const tag = await found.evaluate((element) => element.tagName);
  return tag === "INPUT" || tag === "TEXTAREA" ? found : found.locator("input, textarea").first();
}

const matches = (text: string, label: string | RegExp) => (typeof label === "string" ? text.trim() === label : label.test(text));

/**
 * Chooses `label` in a choice control, whatever its kind: a native select, a text combo, the
 * option itself, a segmented group or tab list holding it, or a picker that opens a list.
 */
export async function choose(page: Page, choice: Locator, label: string | RegExp): Promise<void> {
  const { tag, role, text } = await choice.evaluate((element) => ({ tag: element.tagName, role: element.getAttribute("role"), text: element.textContent ?? "" }));
  if (tag === "SELECT") {
    const options = await choice.locator("option").allTextContents();
    const match = options.find((option) => matches(option, label));
    assert.ok(match !== undefined, `the select offers ${String(label)}: ${options.join(", ")}`);
    await choice.selectOption({ label: match });
    return;
  }
  if (tag === "INPUT" || tag === "TEXTAREA") {
    assert.equal(typeof label, "string", "a text combo is typed into with a string");
    await choice.fill(label as string);
    const suggested = page.getByRole("option", { name: label }).first();
    if (await suggested.isVisible()) await suggested.click();
    return;
  }
  if ((tag === "BUTTON" || ["tab", "radio", "option", "menuitem"].includes(role ?? "")) && matches(text, label)) {
    await choice.click();
    return;
  }
  const inside = choice.getByRole("radio", { name: label }).or(choice.getByRole("tab", { name: label }))
    .or(choice.getByRole("button", { name: label })).or(choice.getByRole("option", { name: label })).first();
  if (await inside.count() > 0) {
    await inside.click();
    return;
  }
  await choice.click();
  const listed = page.getByRole("option", { name: label }).or(page.getByRole("menuitem", { name: label })).first();
  await expect(listed, `the picker lists ${String(label)}`).toBeVisible();
  await listed.click();
}

/** Goes to a screen by its nav entry when the rail shows one, else by its hash route. */
export async function open(page: Page, navLabel: string | RegExp, hash: string): Promise<void> {
  assert.ok(hash.startsWith("#/"), `a screen's route is a hash route: ${hash}`);
  const link = page.getByRole("navigation").getByRole("link", { name: navLabel }).first();
  if (await link.count() > 0 && await link.isVisible()) {
    await link.click();
    return;
  }
  await page.evaluate((next) => { location.hash = next; }, hash);
}

/** Presses the in-page confirm dialog's `confirmLabel` button when the control just asked for one. */
export async function confirmIfAsked(page: Page, confirmLabel: string | RegExp, timeoutMs = 3_000): Promise<void> {
  const dialog = page.getByRole("dialog").filter({ has: page.getByRole("button", { name: confirmLabel }) }).last();
  try {
    await dialog.waitFor({ state: "visible", timeout: timeoutMs });
  } catch (error) {
    // No dialog is an expected answer: the control ran without asking.
    if (error instanceof errors.TimeoutError) return;
    throw error;
  }
  await dialog.getByRole("button", { name: confirmLabel }).last().click();
  await expect(dialog, "the confirm dialog closes").toHaveCount(0);
}
