// Steps more than one journey takes: the start sheet, landing on an agent, its controls, and
// stopping then resuming it. Each finds the new UI's control first and today's after.

import assert from "node:assert/strict";
import { errors, expect, type Locator, type Page } from "@playwright/test";
import { until } from "../../harness/process.ts";
import type { Stack } from "../../harness/stack.ts";
import { aetherConn, route } from "../flows/desktop/state.ts";
import { agent, agentIds } from "./daemon.ts";
import type { Variant } from "./journey.ts";
import { choose, confirmIfAsked, isTestId, need, note, open, textField } from "./ui.ts";

const AGENT_MS = 30_000;
const LAND_MS = 5_000;

export const startSheet = (page: Page): Locator => page.getByRole("dialog", { name: "Start an agent" });
export const agentPage = (page: Page): Locator => page.getByRole("region", { name: /^Agent / });

/** Activity's start sheet, open: the new UI's [start-agent], else today's "Start an agent". */
export async function openStart(page: Page): Promise<void> {
  await open(page, "Activity", "#/activity");
  const start = await need(page, "start-agent", (p) => p.getByRole("button", { name: "Start an agent" }));
  await start.click();
  await expect(startSheet(page).or(page.getByTestId("start-objective")).first(), "the start sheet opens").toBeVisible();
}

export async function objectiveField(page: Page): Promise<Locator> {
  return textField(await need(page, "start-objective", (p) => startSheet(p).getByLabel("Objective", { exact: true })));
}

/** Accepts the new UI's default workspace; today's sheet needs one typed and validated. */
export async function chooseWorkspace(page: Page, work: string): Promise<void> {
  assert.ok(work.startsWith("/"), `a workspace is absolute: ${work}`);
  const found = await need(page, "start-workspace", (p) => startSheet(p).getByLabel("Workspace", { exact: true }));
  const field = await textField(found);
  if (await isTestId(found, "start-workspace")) {
    if ((await field.inputValue()) === "") await field.fill(work);
    await expect(field, "the start sheet holds a workspace").not.toHaveValue("");
    return;
  }
  await field.fill(work);
  await startSheet(page).getByRole("button", { name: "Validate workspace" }).click();
  await expect(startSheet(page).getByRole("status").filter({ hasText: "Workspace ready" }), "the daemon accepts the workspace").toBeVisible();
}

/** The variant's model is the one preselected: the faux model, or Haiku on real engines. */
export async function expectDefaultModel(page: Page, variant: Variant): Promise<void> {
  const name = variant === "faux" ? /Faux|mock/i : /Haiku/i;
  const found = await need(page, "start-model", (p) => startSheet(p).getByRole("button", { pressed: true }).filter({ hasText: name }));
  const tag = await found.evaluate((element) => element.tagName);
  if (tag === "SELECT") await expect(found.locator("option:checked"), "the default model is preselected").toHaveText(name);
  else await expect(found, "the default model is preselected").toContainText(name);
}

/** Submits the start sheet; true when by the new UI's [start-submit]. */
export async function submitStart(page: Page, label: "Start agent" | "Create task"): Promise<boolean> {
  const submit = await need(page, "start-submit", (p) => startSheet(p).getByRole("button", { name: label, exact: true }));
  await expect(submit, "the start sheet is ready to submit").toBeEnabled();
  await submit.click();
  await confirmIfAsked(page, label);
  return isTestId(submit, "start-submit");
}

/** The one agent the daemon holds that `before` did not. */
export async function newAgentId(stack: Stack, before: readonly string[]): Promise<string> {
  const ids = await until(() => agentIds(stack), (now) => now.some((id) => !before.includes(id)), "the started agent", AGENT_MS);
  const added = ids.filter((id) => !before.includes(id));
  assert.equal(added.length, 1, `one agent was started: ${added.join(", ")}`);
  return added[0] as string;
}

/** The agent's page: where the new UI lands after starting, else reached by its route. */
export async function landOnAgent(page: Page, stack: Stack, id: string, mustLand: boolean): Promise<void> {
  const landed = await page.waitForURL(new RegExp(`#/agents/[^/]+/${id}$`), { timeout: LAND_MS }).then(() => true, (error: unknown) => {
    if (error instanceof errors.TimeoutError) return false;
    throw error;
  });
  assert.ok(landed || !mustLand, "starting from [start-submit] lands on the agent's page");
  if (!landed) {
    note("fallback", "agent page: reached by its route, as today's start sheet stays on Activity");
    await route(page, `#/agents/${encodeURIComponent(await aetherConn(stack))}/${id}`);
  }
  await expect(agentPage(page), "the agent's page shows").toBeVisible();
}

/** The agent's Controls tab `tab`, through its page's Actions menu or else its route. */
export async function openControls(page: Page, stack: Stack, id: string, tab: string): Promise<Locator> {
  const actions = agentPage(page).getByRole("button", { name: "Actions" });
  if (await actions.isVisible()) {
    await actions.click();
    await page.getByRole("menuitem", { name: "Open controls" }).click();
  } else {
    await route(page, `#/controls/${encodeURIComponent(await aetherConn(stack))}/${id}`);
  }
  const screen = page.getByRole("region", { name: /^Controls for / });
  await expect(screen, "the agent's controls show").toBeVisible();
  await choose(page, screen.getByRole("tablist", { name: "Agent controls" }), tab);
  return screen;
}

async function waitPaused(stack: Stack, id: string, paused: boolean): Promise<void> {
  const view = await until(() => agent(stack, id), (now) => now.paused === paused, `the agent to be ${paused ? "paused" : "resumed"}`, AGENT_MS);
  assert.equal(view.paused, paused);
}

/** The entity action menu's item `name` on the agent's page, when it has one. */
async function menuAction(page: Page, name: RegExp): Promise<Locator | null> {
  const actions = agentPage(page).getByRole("button", { name: "Actions" });
  await expect(actions, "the agent's page has an entity action menu").toBeVisible();
  await actions.click();
  const item = page.getByRole("menuitem", { name }).first();
  if (await item.isVisible()) return item;
  await page.keyboard.press("Escape");
  return null;
}

/** Stops then resumes the agent: from its action menu in the new UI, else from Controls › Lifecycle. */
export async function stopAndResume(page: Page, stack: Stack, id: string): Promise<void> {
  const stop = await menuAction(page, /^(Stop|Pause)\b/);
  if (stop !== null) {
    await stop.click();
    await confirmIfAsked(page, /^(Stop|Pause)\b/);
    await waitPaused(stack, id, true);
    const resume = await menuAction(page, /^Resume\b/);
    assert.ok(resume !== null, "a stopped agent's menu offers Resume");
    await resume.click();
    await confirmIfAsked(page, /^Resume\b/);
    await waitPaused(stack, id, false);
    return;
  }
  note("fallback", "stop/resume: Controls › Lifecycle, as today's action menu has no Stop");
  const controls = await openControls(page, stack, id, "Lifecycle");
  await controls.getByRole("button", { name: "Pause", exact: true }).click();
  await confirmIfAsked(page, "Pause");
  await waitPaused(stack, id, true);
  await controls.getByRole("button", { name: "Resume", exact: true }).click();
  await confirmIfAsked(page, "Resume");
  await waitPaused(stack, id, false);
}
