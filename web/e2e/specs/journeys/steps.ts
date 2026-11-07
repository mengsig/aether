// Steps more than one journey takes: the start sheet, landing on an agent, its controls, and
// stopping then resuming it.

import assert from "node:assert/strict";
import { expect, type Locator, type Page } from "@playwright/test";
import { until } from "../../harness/process.ts";
import type { Stack } from "../../harness/stack.ts";
import { aetherConn, route } from "../flows/desktop/state.ts";
import { agent, agentIds } from "./daemon.ts";
import type { Variant } from "./journey.ts";
import { shootWindow } from "./capture.ts";
import { choose, need, open, textField } from "./ui.ts";

const AGENT_MS = 30_000;
const LAND_MS = 10_000;

export const startSheet = (page: Page): Locator => page.getByRole("dialog", { name: "Start an agent" });
export const agentPage = (page: Page): Locator => page.getByRole("region", { name: /^Agent / });

/** Activity's start sheet, open from the connection's "Start an agent". */
export async function openStart(page: Page): Promise<void> {
  await open(page, "Activity", "#/activity");
  const start = page.getByRole("button", { name: "Start an agent", exact: true }).first();
  await expect(start, "Activity offers Start an agent").toBeVisible();
  await start.click();
  await expect(startSheet(page), "the start sheet opens").toBeVisible();
}

/** The sheet's work type: "Chat" (the default), "Task agent" or "Existing agent task". */
export async function chooseWorkType(page: Page, type: "Chat" | "Task agent" | "Existing agent task"): Promise<void> {
  const tabs = (await need(startSheet(page), "start-mode")).getByRole("tablist", { name: "Work type" });
  await choose(page, tabs, type);
  await expect(tabs.getByRole("tab", { name: type, exact: true }), `the sheet is set to ${type}`).toHaveAttribute("aria-selected", "true");
}

export async function objectiveField(page: Page): Promise<Locator> {
  return textField(await need(startSheet(page), "start-objective"));
}

/** The workspace field holds `work`: typed over the sheet's default, then validated by the daemon. */
export async function chooseWorkspace(page: Page, work: string): Promise<void> {
  assert.ok(work.startsWith("/"), `a workspace is absolute: ${work}`);
  const sheet = startSheet(page);
  const field = await textField(await need(sheet, "start-workspace"));
  await field.fill(work);
  await expect(field, "the start sheet holds the workspace").toHaveValue(work);
  await expect(sheet.getByRole("status").filter({ hasText: /^Ready/ }), "the daemon accepts the workspace").toBeVisible();
}

/** The variant's model is the one preselected: the faux model, or Haiku on real engines. */
export async function expectDefaultModel(page: Page, variant: Variant): Promise<void> {
  const name = variant === "faux" ? /Faux model/i : /Haiku/i;
  await expect(await need(startSheet(page), "start-model"), "the default model is preselected").toContainText(name);
}

/** Submits the start sheet by [start-submit]; it closes and lands on the agent. */
export async function submitStart(page: Page, label: "Start agent" | "Create task"): Promise<void> {
  const submit = await need(startSheet(page), "start-submit");
  await expect(submit, `the start sheet submits with ${label}`).toHaveText(label);
  await expect(submit, "the start sheet is ready to submit").toBeEnabled();
  await submit.click();
  await expect(startSheet(page), "submitting closes the start sheet").toHaveCount(0);
}

/** The one agent the daemon holds that `before` did not. */
export async function newAgentId(stack: Stack, before: readonly string[]): Promise<string> {
  const ids = await until(() => agentIds(stack), (now) => now.some((id) => !before.includes(id)), "the started agent", AGENT_MS);
  const added = ids.filter((id) => !before.includes(id));
  assert.equal(added.length, 1, `one agent was started: ${added.join(", ")}`);
  return added[0] as string;
}

/** The agent's own route: Controls lives at `#/controls/<conn>/<id>`, so the id alone does not tell them apart. */
function agentRoute(id: string): RegExp {
  assert.match(id, /^\S+$/, "an agent id has no spaces");
  return new RegExp(`#/agents/[^/]+/${id}$`);
}

/** Starting lands on the agent's page. */
export async function landOnAgent(page: Page, id: string): Promise<void> {
  await page.waitForURL(agentRoute(id), { timeout: LAND_MS });
  await expect(agentPage(page), "the agent's page shows").toBeVisible();
}

/** Opens the agent's own page (its conversation) from wherever the window is. */
export async function openConversation(page: Page, stack: Stack, id: string): Promise<void> {
  if (!agentRoute(id).test(page.url())) await route(page, `#/agents/${encodeURIComponent(await aetherConn(stack))}/${id}`);
  await landOnAgent(page, id);
}

/** The agent's Controls tab `tab`, through its page's Actions menu › Open controls. */
export async function openControls(page: Page, stack: Stack, id: string, tab: string): Promise<Locator> {
  const view = await agent(stack, id);
  await openConversation(page, stack, id);
  const actions = agentPage(page).getByRole("button", { name: "Actions", exact: true });
  await expect(actions, "the agent's page has an entity action menu").toBeVisible();
  await actions.click();
  await page.getByRole("menuitem", { name: "Open controls" }).click();
  const screen = page.getByRole("region", { name: `Controls for ${view.name}` });
  await expect(screen, "the agent's controls show").toBeVisible();
  await choose(page, screen.getByRole("tablist", { name: "Agent controls" }), tab);
  return screen;
}

async function waitPaused(stack: Stack, id: string, paused: boolean): Promise<void> {
  const view = await until(() => agent(stack, id), (now) => now.paused === paused, `the agent to be ${paused ? "paused" : "resumed"}`, AGENT_MS);
  assert.equal(view.paused, paused);
}

/** Stops then resumes the agent from Controls › Lifecycle, its one Pause/Resume control. */
export async function stopAndResume(page: Page, stack: Stack, id: string): Promise<void> {
  const controls = await openControls(page, stack, id, "Lifecycle");
  await controls.getByRole("button", { name: "Pause", exact: true }).click();
  await waitPaused(stack, id, true);
  await shootWindow(page, "controls", "lifecycle-paused");
  await controls.getByRole("button", { name: "Resume", exact: true }).click();
  await waitPaused(stack, id, false);
}

/** Finds `text` through the conversation's own Find: the current match is scrolled into view. */
export async function findInConversation(page: Page, text: string): Promise<Locator> {
  assert.ok(text.trim() !== "", "a find looks for text");
  const conversation = agentPage(page);
  await conversation.getByRole("button", { name: "Find" }).click();
  await (await need(conversation, "conv-search")).fill(text);
  await expect(await need(conversation, "conv-search-count"), "the text is found once").toHaveText(/^1 of 1$/);
  const current = conversation.locator('.conv-msg[data-current="true"]');
  await expect(current, "the match is drawn").toContainText(text);
  return current;
}
