import assert from "node:assert/strict";
import path from "node:path";
import { enqueueItem } from "../../../harness/agents.ts";
import { DEFAULT_REPLY } from "../../../harness/engine.ts";
import { expect, test, type Page } from "../../../harness/fixtures.ts";
import { connection } from "../../../harness/pair.ts";
import { fixtureRepository } from "../gh-stub.ts";
import { startPrivateStack, stopPrivateStack, type PrivateStack } from "../private.ts";
import { launchDesktop } from "./electron.ts";
import { launchPackaged, packagedBinary } from "./packaged.ts";
import { itemState, route, setPaused } from "./state.ts";

const TITLE = "Desktop completed ask journey";
const SUMMARY = "The requested answer is ready for review.";
const INITIATIVE = "Desktop two member journey";
const CONTRACT = "Both members share the same outcome.";
let running: PrivateStack | undefined;
let app: { readonly window: Page; close(): Promise<void> } | undefined;
const errors: string[] = [];

async function shots(page: Page, name: string): Promise<void> {
  const dir = process.env.E2E_SHOTS;
  if (!dir) return;
  assert.ok(path.isAbsolute(dir), "E2E_SHOTS is an absolute directory");
  for (const theme of ["light", "dark"] as const) {
    await page.evaluate((value) => { document.documentElement.dataset.theme = value; }, theme);
    await page.screenshot({ path: path.join(dir, `${name}-${theme}.png`) });
  }
}

test.beforeAll(async () => {
  test.setTimeout(180_000);
  running = await startPrivateStack();
  const { aether, archon } = running.stack.layout;
  const binary = packagedBinary();
  app = binary ? await launchPackaged(binary, { aether: aether.socket, archon: archon.socket })
    : await launchDesktop({ aetherSocket: aether.socket, archonSocket: archon.socket });
  app.window.on("pageerror", (error) => errors.push(error.message));
  await app.window.setViewportSize({ width: 1440, height: 900 });
  await expect(connection(app.window, "archon")).toHaveAttribute("data-state", "ready", { timeout: 30_000 });
});

test.afterAll(async () => {
  try { await app?.close(); } finally { await stopPrivateStack(running); }
});

test("completed ask run and a two-member initiative render from daemon state", async () => {
  test.setTimeout(180_000);
  assert.ok(running && app, "the private stack and desktop are running");
  const { stack } = running;
  const page = app.window;
  stack.engine.setScript((request) => request.lastUserText.includes(TITLE)
    ? { name: "complete_task", arguments: { summary: SUMMARY, structured: { answer: "Ready", evidence: ["First", "Second"] } } }
    : DEFAULT_REPLY);
  const item = await enqueueItem(stack, TITLE);
  await expect.poll(() => itemState(stack, item), { timeout: 120_000, message: "the ask item succeeds" }).toBe("succeeded");
  const detail = await stack.archon.request({ cmd: "get_item", item });
  assert.equal(detail.reply, "item_detail");
  const runs = detail.data.runs as Array<{ id: string }>;
  assert.ok(Array.isArray(runs) && runs.length > 0, "the completed item has a run");
  assert.ok(runs[0]?.id, "the run has an id");
  await route(page, `#/archon/items/${item}`);
  await expect(page.locator("#item-title")).toHaveText(TITLE);
  await expect(page.getByRole("link", { name: /Run 1 · ask/ })).toBeVisible();
  await shots(page, "completed-item");
  await route(page, `#/archon/items/${item}/runs/${runs[0].id}`);
  await expect(page.locator("#pipeline-title")).toHaveText("Run 1 · ask");
  await expect(page.locator(".pipeline-attempts")).toContainText("answer");
  await expect(page.locator(".pipeline-summary")).toContainText(/requested answer|Ready/);
  await shots(page, "completed-run");
  await setPaused(stack, true);
  const added = await stack.archon.request({ cmd: "repo_add", path: await fixtureRepository(stack.layout), name: "journey-repo" });
  assert.notEqual(added.reply, "error");
  const members = ["First", "Second"].map((name) => ({ repo: "journey-repo", title: `${name} member`, brief: `${name} member work.` }));
  const made = await stack.archon.request({ cmd: "enqueue_initiative", title: INITIATIVE, contract: CONTRACT, members });
  assert.equal(made.reply, "initiative");
  const id = String((made.data.initiative as { id?: unknown } | undefined)?.id ?? made.data.id);
  assert.match(id, /^in_/);
  await route(page, `#/archon/initiatives/${id}`);
  await expect(page.locator(".initiative-title")).toHaveText(INITIATIVE);
  await expect(page.getByRole("region", { name: "Contract" })).toContainText(CONTRACT);
  await expect(page.getByRole("region", { name: "Members" }).locator(".dag-count")).toHaveText("2");
  await shots(page, "two-member-initiative");
  expect(errors, "the desktop threw no page errors").toEqual([]);
});
