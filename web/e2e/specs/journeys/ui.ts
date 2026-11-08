// Finding the journeys' controls: by the screens' `data-testid` where they carry one, else by role
// or label. A control that does not show within the bound fails the step, naming it.

import assert from "node:assert/strict";
import { errors, expect, test, type Locator, type Page } from "@playwright/test";

const FIND_MS = 8_000;

/** An annotation on the test's report: a step the journey took one way of several. */
export function note(type: string, description: string): void {
  test.info().annotations.push({ type, description });
}

/** The visible control with test id `id`, within `scope` (the page by default). */
export async function need(scope: Page | Locator, id: string, timeoutMs = FIND_MS): Promise<Locator> {
  assert.match(id, /^[a-z][a-z0-9-]*$/, `a test id is kebab-case: ${id}`);
  const found = scope.getByTestId(id).first();
  await expect(found, `[data-testid=${id}] shows`).toBeVisible({ timeout: timeoutMs });
  return found;
}

/** Whether `found` shows within the bound; absent is an expected answer for the caller. */
export async function appears(found: Locator, timeoutMs = FIND_MS): Promise<boolean> {
  try {
    await found.first().waitFor({ state: "visible", timeout: timeoutMs });
    return true;
  } catch (error) {
    if (error instanceof errors.TimeoutError) return false;
    throw error;
  }
}

/** The text field itself, or the one a test-id wrapper holds. */
export async function textField(found: Locator): Promise<Locator> {
  const tag = await found.evaluate((element) => element.tagName);
  const field = tag === "INPUT" || tag === "TEXTAREA" ? found : found.locator("input, textarea").first();
  await expect(field, "the text field is editable").toBeEditable();
  return field;
}

const matches = (text: string, label: string | RegExp) => (typeof label === "string" ? text.trim() === label : label.test(text));

/** A Picker (role=combobox button): left alone when it already shows `label`, else opened and chosen. */
async function pick(page: Page, picker: Locator, label: string | RegExp): Promise<void> {
  const value = picker.locator(".ui-picker-value");
  if (matches(await value.textContent() ?? "", label)) return;
  await picker.click();
  const listed = page.getByRole("listbox").getByRole("option", { name: label }).first();
  await expect(listed, `the picker lists ${String(label)}`).toBeVisible();
  await listed.click();
  await expect(value, `the picker shows ${String(label)}`).toHaveText(label);
}

/**
 * Chooses `label` in a choice control, whatever its kind: a Picker, a text combo, the option
 * itself, or a segmented group or tab list holding it.
 */
export async function choose(page: Page, choice: Locator, label: string | RegExp): Promise<void> {
  const { tag, role, text } = await choice.evaluate((element) => ({ tag: element.tagName, role: element.getAttribute("role"), text: element.textContent ?? "" }));
  if (role === "combobox" && tag === "BUTTON") return pick(page, choice, label);
  if (tag === "INPUT" || tag === "TEXTAREA") {
    assert.equal(typeof label, "string", "a text combo is typed into with a string");
    await choice.fill(label as string);
    const suggested = page.getByRole("option", { name: label, exact: true }).first();
    if (await suggested.isVisible()) await suggested.click();
    await expect(choice, "the combo holds the choice").toHaveValue(label as string);
    return;
  }
  if (["tab", "radio", "option", "menuitem"].includes(role ?? "") && matches(text, label)) {
    await choice.click();
    return;
  }
  const inside = choice.getByRole("radio", { name: label }).or(choice.getByRole("tab", { name: label })).first();
  await expect(inside, `${role ?? tag} offers ${String(label)}`).toHaveCount(1);
  await inside.click();
  await expect(inside, `${String(label)} is chosen`).toHaveAttribute(role === "tablist" ? "aria-selected" : "aria-checked", "true");
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

/**
 * Presses `confirmLabel` in the confirmation an Action just opened (controls/kit.tsx: "Keep it" and
 * the action), when it asked; a sheet holding the control itself is never mistaken for one.
 */
export async function confirmIfAsked(page: Page, confirmLabel: string | RegExp, timeoutMs = 3_000): Promise<void> {
  const dialog = page.getByRole("dialog").filter({ has: page.getByRole("button", { name: "Keep it", exact: true }) })
    .filter({ has: page.getByRole("button", { name: confirmLabel }) }).last();
  if (!(await appears(dialog, timeoutMs))) return;
  await dialog.getByRole("button", { name: confirmLabel }).last().click();
  await expect(dialog, "the confirm dialog closes").toHaveCount(0);
}
