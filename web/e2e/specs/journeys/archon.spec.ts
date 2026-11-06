// Journey 6, Archon: a scratch checkout with no origin registered to keep its work local, an item
// queued on it, a bounded wait for an outcome, then the task graph and the item's history. The
// orchestrator runs on the faux engine.

import assert from "node:assert/strict";
import { expect, test, type Page } from "@playwright/test";
import { until } from "../../harness/process.ts";
import type { Stack } from "../../harness/stack.ts";
import { itemHistory, items, itemState, setPaused } from "../flows/desktop/state.ts";
import { scratchRepo } from "./daemon.ts";
import { connectionsReady, journeyHooks } from "./journey.ts";
import { choose, need, open, textField } from "./ui.ts";

const journey = journeyHooks();
const REPO = "journey-scratch";
const TITLE = "Journey item: say hello";
const BRIEF = "Answer with one short greeting, then finish.";
const OUTCOME_MS = 150_000;
const ACTIVE = new Set(["queued", "dispatching", "spawning", "running", "ready"]);

interface HeldRepo { readonly name: string; readonly ship: string }

async function heldRepos(stack: Stack): Promise<HeldRepo[]> {
  const { reply, data } = await stack.archon.request({ cmd: "repo_list" });
  assert.equal(reply, "repos", `repo_list answers repos: ${reply}`);
  const rows = (Array.isArray(data) ? data : (data as { repos?: unknown }).repos) as Array<{ name?: unknown; ship?: unknown }>;
  assert.ok(Array.isArray(rows), "repo_list lists the repositories");
  return rows.map((repo) => ({ name: String(repo.name ?? ""), ship: String(repo.ship ?? "") }));
}

/** Registers the checkout from the Queue's empty state to keep its work local; that opens New item on it. */
async function registerRepo(page: Page, stack: Stack, repo: string): Promise<void> {
  await open(page, "Queue", "#/archon/queue");
  await (await need(page, "repo-register")).click();
  const sheet = page.getByRole("dialog", { name: "Register a repository" });
  await expect(sheet, "the register sheet opens").toBeVisible();
  await (await textField(await need(sheet, "repo-path"))).fill(repo);
  const name = await textField(await need(sheet, "repo-name"));
  await expect(name, "the name follows the folder").toHaveValue(REPO);
  await choose(page, sheet.getByRole("radiogroup", { name: "When work is done" }), "Keep it local");
  await expect(sheet.getByRole("switch", { name: "Watch GitHub" }), "the desktop leaves watching GitHub to the archon CLI").toBeDisabled();
  const submit = await need(sheet, "repo-register-submit");
  await expect(submit, "the repository is ready to register").toBeEnabled();
  await submit.click();
  await expect(sheet, "registering closes the sheet").toHaveCount(0);
  const repos = await until(() => heldRepos(stack), (now) => now.some((held) => held.name === REPO), "the registered repository", 30_000);
  const held = repos.filter((repo) => repo.name === REPO);
  assert.equal(held.length, 1, "archond holds the repository once");
  assert.equal(held[0]?.ship, "local", "archond keeps the repository's work local");
}

/** Fills the New item dialog registering opened, on the new repository, and queues it. */
async function queueItem(page: Page): Promise<void> {
  const dialog = page.getByRole("dialog", { name: "New item" });
  await expect(dialog, "registering opens New item").toBeVisible();
  await (await textField(await need(dialog, "new-item-title"))).fill(TITLE);
  await (await textField(await need(dialog, "new-item-brief"))).fill(BRIEF);
  await expect((await need(dialog, "new-item-repo")).locator(".ui-picker-value"), "New item is on the new repository").toHaveText(REPO);
  await choose(page, await need(dialog, "new-item-pipeline"), /^ask$/);
  const submit = await need(dialog, "new-item-submit");
  await expect(submit, "the item is ready to queue").toBeEnabled();
  await submit.click();
  await expect(dialog, "queueing closes the dialog").toHaveCount(0);
}

test("archon journey: register a repository, queue an item, see it through", async () => {
  const { window, stack } = journey();
  await connectionsReady(journey());
  await setPaused(stack, false);
  await registerRepo(window, stack, await scratchRepo(stack, REPO));
  await queueItem(window);
  const item = await until(() => items(stack), (rows) => rows.some((row) => row.title === TITLE), "the queued item", 30_000);
  const id = item.find((row) => row.title === TITLE)?.id as string;
  const outcome = await until(() => itemState(stack, id), (state) => !ACTIVE.has(state), "the item to reach an outcome", OUTCOME_MS);
  test.info().annotations.push({ type: "outcome", description: `${TITLE}: ${outcome}` });
  assert.ok((await itemHistory(stack, id)).length >= 2, "the item moved at least once");
  await open(window, "Task graph", "#/archon/graph");
  await expect(window.getByRole("complementary", { name: "Graph overview" }).or(window.getByText("Nothing in the graph")).first(), "the task graph renders").toBeVisible();
  await open(window, "History", "#/archon/history");
  const row = window.getByRole("list", { name: "Items" }).getByText(TITLE).first();
  await expect(row, "History lists the item").toBeVisible();
  await row.click();
  await expect(window.getByRole("complementary", { name: "Item history" }), "the item's history opens").toBeVisible();
});
