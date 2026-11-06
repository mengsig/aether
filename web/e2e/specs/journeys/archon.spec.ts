// Journey 6, Archon: a scratch repository registered, an item queued on it, a bounded wait for
// an outcome, then the task graph and the item's history. The orchestrator runs on the faux engine.

import assert from "node:assert/strict";
import { expect, test, type Page } from "@playwright/test";
import { until } from "../../harness/process.ts";
import type { Stack } from "../../harness/stack.ts";
import { itemHistory, items, itemState, setPaused } from "../flows/desktop/state.ts";
import { scratchRepo } from "./daemon.ts";
import { connectionsReady, journeyHooks } from "./journey.ts";
import { choose, finish, need, open, optional, shows, textField } from "./ui.ts";

const journey = journeyHooks();
const REPO = "journey-scratch";
const TITLE = "Journey item: say hello";
const BRIEF = "Answer with one short greeting, then finish.";
const OUTCOME_MS = 150_000;
const ACTIVE = new Set(["queued", "dispatching", "spawning", "running", "ready"]);

async function repoNames(stack: Stack): Promise<string[]> {
  const { reply, data } = await stack.archon.request({ cmd: "repo_list" });
  assert.equal(reply, "repos", `repo_list answers repos: ${reply}`);
  const rows = (Array.isArray(data) ? data : (data as { repos?: unknown }).repos) as Array<{ name?: string }>;
  assert.ok(Array.isArray(rows), "repo_list lists the repositories");
  return rows.map((repo) => String(repo.name ?? ""));
}

/** Registers the repository from the window; without the new UI's form, from the host. */
async function registerRepo(page: Page, stack: Stack, repo: string): Promise<void> {
  const fromWindow = await optional(page, "repo-register", undefined, async (register) => {
    await register.click();
    await (await textField(await need(page, "repo-path"))).fill(repo);
    await (await need(page, "repo-register-submit")).click();
  });
  if (!fromWindow) {
    const added = await stack.archon.request({ cmd: "repo_add", path: repo, name: REPO });
    assert.notEqual(added.reply, "error", `archond registers the scratch repository: ${JSON.stringify(added.data)}`);
  }
  const names = await until(() => repoNames(stack), (now) => now.some((name) => name.includes(REPO)), "the registered repository", 30_000);
  assert.ok(names.length >= 1, "archond holds the repository");
}

async function queueItem(page: Page): Promise<void> {
  await open(page, "Queue", "#/archon/queue");
  await (await need(page, "queue-new-item", (p) => p.getByRole("button", { name: "New item", exact: true }))).click();
  const dialog = page.getByRole("dialog", { name: "New item" });
  await (await textField(await need(page, "new-item-title", () => dialog.getByLabel("Title", { exact: true })))).fill(TITLE);
  await (await textField(await need(page, "new-item-brief", () => dialog.getByLabel("Brief", { exact: true })))).fill(BRIEF);
  await choose(page, await need(page, "new-item-repo", () => dialog.getByLabel("Repository", { exact: true })), new RegExp(REPO));
  await choose(page, await need(page, "new-item-pipeline", () => dialog.getByLabel("Pipeline", { exact: true })), "ask");
  await (await need(page, "new-item-submit", () => dialog.getByRole("button", { name: "Queue item", exact: true }))).click();
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
  if (await shows(row, "the item in History")) {
    await row.click();
    await expect(window.getByRole("complementary", { name: "Item history" }), "the item's history opens").toBeVisible();
  }
  finish();
});
