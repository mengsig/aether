// The design audit's screenshots: every screen and its main states, desktop and narrow, light and
// dark, from the built frontend on the e2e stack. Runs only when E2E_CAPTURE names an output dir.

import assert from "node:assert/strict";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { createAgent, enqueueItem, pauseQueue } from "../../harness/agents.ts";
import { pairBoth } from "../../harness/pair.ts";
import { until } from "../../harness/process.ts";
import { expect, test, type Page } from "../../harness/fixtures.ts";
import type { Stack } from "../../harness/stack.ts";
import { fixtureRepository, installGhStub, OWNER_REPO } from "../flows/gh-stub.ts";
import { startPrivateStack, stopPrivateStack } from "../flows/private.ts";
import { configureNoesis, openAgent } from "../noesis/noesis.ts";

const OUT = process.env.E2E_CAPTURE;
const VARIANTS = [
  { name: "desktop-light", width: 1280, height: 800, scheme: "light" },
  { name: "desktop-dark", width: 1280, height: 800, scheme: "dark" },
  { name: "narrow-light", width: 390, height: 844, scheme: "light" },
  { name: "narrow-dark", width: 390, height: 844, scheme: "dark" },
] as const;
const NO_ITEM = `qi_${"0".repeat(26)}`;
const NO_AGENT = `agt_${"0".repeat(26)}`;
const SETTLE_MS = 350;
/** archond reads GitHub on a sync that sleeps 60 s between passes, so a GitHub page can take that long. */
const GH_SYNC_MS = 75_000;

test.skip(OUT === undefined, "E2E_CAPTURE names the directory the audit's screenshots go to");
test.describe.configure({ mode: "serial" });

interface Shot { readonly screen: string; readonly state: string; readonly file: string; readonly variants: readonly string[] }
const manifest: Shot[] = [];

test.afterAll(async () => {
  if (OUT === undefined) return;
  await writeFile(path.join(OUT, "manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
});

interface ShootOptions {
  readonly ready?: string;
  readonly readyMs?: number;
  /** Runs after the screen is open, before the shots: opens a sheet, expands a row. */
  readonly prepare?: (page: Page) => Promise<void>;
}

/** Opens `hash`, waits for `ready` (or for the screen's chunk), then shoots it in every variant. */
async function shoot(page: Page, stack: Stack, screen: string, state: string, hash: string, readyOrOptions?: string | ShootOptions, readyMs = 15_000): Promise<void> {
  const options: ShootOptions = typeof readyOrOptions === "string" ? { ready: readyOrOptions, readyMs } : readyOrOptions ?? {};
  const ready = options.ready;
  const waitMs = options.readyMs ?? readyMs;
  assert.ok(OUT !== undefined, "E2E_CAPTURE is set");
  assert.ok(hash.startsWith("#/") || hash.startsWith("/"), `a route or path: ${hash}`);
  await mkdir(OUT, { recursive: true });
  await page.goto(`${stack.aether.origin}${hash.startsWith("#") ? `/${hash}` : hash}`);
  await expect(page.locator(".ui-screen-skeleton"), `${screen} loads its screen`).toHaveCount(0);
  // Off every control, so no hover style shows in the shot.
  await page.mouse.move(0, 0);
  if (ready !== undefined) await expect(page.locator(ready).first(), `${screen} (${state}) shows ${ready}`).toBeVisible({ timeout: waitMs });
  if (options.prepare !== undefined) await options.prepare(page);
  const file = `${String(manifest.length + 1).padStart(2, "0")}-${screen}-${state}`.toLowerCase().replace(/[^a-z0-9-]+/g, "-");
  for (const variant of VARIANTS) {
    await page.setViewportSize({ width: variant.width, height: variant.height });
    await page.emulateMedia({ colorScheme: variant.scheme, reducedMotion: "reduce" });
    await page.waitForTimeout(SETTLE_MS);
    await page.screenshot({ path: path.join(OUT, `${file}--${variant.name}.png`), fullPage: true });
  }
  manifest.push({ screen, state, file, variants: VARIANTS.map((variant) => variant.name) });
}

async function openStartSheet(page: Page): Promise<void> {
  await page.getByRole("button", { name: "Start an agent" }).first().click();
  await expect(page.getByTestId("start-submit")).toBeVisible();
  await page.getByTestId("start-objective").fill("Read the repository and list the three riskiest files.");
  await page.mouse.move(0, 0);
}

test("unpaired: the shell and both pairing screens", async ({ page, stack }) => {
  await shoot(page, stack, "Home", "unpaired", "#/", '[data-testid="home-dashboard"]');
  await shoot(page, stack, "Fleet", "unpaired", "#/fleet");
  await shoot(page, stack, "Pairing", "Aether", "#/pair/aether", "form");
  await shoot(page, stack, "Pairing", "Archon", "#/pair/archon", "form");
});

test("operator: empty, then loaded, then error states", async ({ page, stack }) => {
  test.setTimeout(300_000);
  await pauseQueue(stack);
  await pairBoth(page, stack, { aether: "operator", archon: "operator" }, "Design audit");
  await shoot(page, stack, "Queue", "empty", "#/archon/queue", ".queue-pause");
  await shoot(page, stack, "Inbox", "empty", "#/inbox");
  await shoot(page, stack, "History", "empty", "#/archon/history");
  await shoot(page, stack, "Task graph", "empty", "#/archon/graph");

  await shoot(page, stack, "Home", "empty", "#/", '[data-testid="home-dashboard"][data-mode="tiles"]');
  await shoot(page, stack, "GitHub overview", "off", "#/github", ".ui-state");
  await shoot(page, stack, "Activity", "start-sheet", "#/activity", { prepare: openStartSheet });

  // The worker's first turn is a scripted shell call, so the conversation shows a tool row and its output. Its second
  // call waits, so the agent is still running, with its composer open, while the agent screens are shot.
  stack.engine.setScript((request) => {
    if (!/Answer briefly/.test(request.lastUserText)) return "Nothing to do; finishing.";
    if (request.toolResults.length === 0) return { toolCall: { name: "bash", arguments: { command: "ls -1" }, text: "Let me look at the workspace first." } };
    if (request.toolResults.length === 1) return { toolCall: { name: "bash", arguments: { command: "sleep 150" }, text: "Checking the files more closely." } };
    return "The workspace holds the fixture files listed above. Nothing else to do; finishing.";
  });
  await createAgent(stack, "Design audit worker", "Answer briefly, then finish.");
  await createAgent(stack, "Idle reviewer", "Wait for work.", false);
  const itemId = await enqueueItem(stack, "Polish the queue rows");
  await enqueueItem(stack, "Tighten the empty states");
  await shoot(page, stack, "Home", "loaded", "#/", '[data-testid="home-dashboard"][data-mode="tiles"]');
  await shoot(page, stack, "Fleet", "loaded", "#/fleet", ".fleet-count");
  await shoot(page, stack, "Topology", "loaded", "#/fleet/topology", '[data-testid="graph-legend"]');
  const conn = await openAgent(page, stack, "Design audit worker");
  const agentId = /agt_[0-9A-Z]{26}/.exec(page.url())?.[0];
  assert.ok(agentId !== undefined, "the agent route names the agent");
  const agentHash = `#/agents/${encodeURIComponent(conn)}/${agentId}`;
  await shoot(page, stack, "Agent", "conversation", agentHash, { ready: '[data-testid="conv-tool-toggle"]', readyMs: 60_000 });
  await shoot(page, stack, "Agent", "tool-open", agentHash, { ready: '[data-testid="conv-tool-toggle"]', prepare: async (p) => {
    await p.getByTestId("conv-tool-toggle").first().click();
    await expect(p.getByTestId("conv-tool-output").first()).toBeVisible();
    await p.getByTestId("composer-input").fill("Thanks. Now summarise what you found in one line.");
    await p.mouse.move(0, 0);
  } });
  await shoot(page, stack, "Controls", "operator", `#/controls/${encodeURIComponent(conn)}/${agentId}`, ".b3-action");
  await shoot(page, stack, "Memory", "review", `#/memory/${encodeURIComponent(conn)}`);
  await shoot(page, stack, "Activity", "loaded", "#/activity");
  await shoot(page, stack, "Queue", "paused", "#/archon/queue", ".queue-row");
  await shoot(page, stack, "Item", "queued", `#/archon/items/${itemId}`, "#item-title");
  await shoot(page, stack, "Task graph", "loaded", "#/archon/graph", '[data-testid="graph-legend"]');
  await shoot(page, stack, "History", "loaded", "#/archon/history");
  await shoot(page, stack, "Queue", "new-item", "#/archon/queue", { ready: ".queue-row", prepare: async (p) => {
    await p.getByRole("button", { name: "New item" }).click();
    await expect(p.getByTestId("new-item-submit")).toBeVisible();
    await p.mouse.move(0, 0);
  } });
  for (const tab of ["connections", "devices", "remote", "providers", "notifications"]) {
    await shoot(page, stack, "Settings", tab, `#/settings/${tab}`, '[role="tabpanel"]');
  }

  await shoot(page, stack, "Item", "error", `#/archon/items/${NO_ITEM}`);
  await shoot(page, stack, "Agent", "error", `#/agents/${encodeURIComponent(conn)}/${NO_AGENT}`);
  await shoot(page, stack, "Pipeline", "error", `#/archon/items/${itemId}/runs/run_missing`);
  await shoot(page, stack, "Initiative", "error", "#/archon/initiatives/ini_missing");
});

test("loading: the screens while their sockets wait", async ({ page, stack }) => {
  await pairBoth(page, stack, { aether: "operator", archon: "operator" }, "Design audit loading");
  // A route that never connects: every daemon socket stays opening, so each screen shows its loading state.
  await page.routeWebSocket(() => true, () => undefined);
  await page.reload();
  await shoot(page, stack, "Fleet", "loading", "#/fleet");
  await shoot(page, stack, "Queue", "loading", "#/archon/queue");
  await shoot(page, stack, "Settings", "loading", "#/settings/devices");
});

test("observer: the read-only controls, queue and item", async ({ page, stack }) => {
  await pauseQueue(stack);
  await pairBoth(page, stack, { aether: "observer", archon: "observer" }, "Design audit observer");
  const conn = await openAgent(page, stack, "Idle reviewer");
  const agentId = /agt_[0-9A-Z]{26}/.exec(page.url())?.[0];
  assert.ok(agentId !== undefined, "the agent route names the agent");
  await shoot(page, stack, "Controls", "observer", `#/controls/${encodeURIComponent(conn)}/${agentId}`, ".b3-action");
  await shoot(page, stack, "Queue", "observer", "#/archon/queue", ".queue-row");
  const items = await stack.archon.request({ cmd: "list_item_summaries" });
  const item = (items.data as unknown as { id: string; title: string }[]).find((row) => row.title === "Polish the queue rows");
  assert.ok(item !== undefined, "the operator test queued the item");
  await shoot(page, stack, "Item", "observer", `#/archon/items/${item.id}`, "#item-title");
});

test("noesis: the learning screens off, then on", async ({ page, stack }) => {
  await pairBoth(page, stack, { aether: "operator", archon: "operator" }, "Design audit noesis");
  await shoot(page, stack, "Learn", "off", "#/learn", '[data-testid="noesis-off"]');
  await shoot(page, stack, "Learn", "settings-sheet", "#/learn", { ready: '[data-testid="noesis-off"]', prepare: async (p) => {
    await p.getByRole("button", { name: "Open Noesis settings" }).click();
    await expect(p.getByTestId("noesis-save")).toBeVisible();
    await p.mouse.move(0, 0);
  } });
  await configureNoesis(stack, { enabled: true, engineering_context: true });
  try {
    await page.reload();
    await shoot(page, stack, "Learn sessions", "empty", "#/learn", '[data-testid="learn-goal"]');
    await shoot(page, stack, "Knowledge", "empty", "#/learn/graph");
    await shoot(page, stack, "My understanding", "empty", "#/learn/me");
    await shoot(page, stack, "Session", "error", `#/learn/sessions/ns_${"0".repeat(26)}`);
  } finally {
    await configureNoesis(stack, { enabled: false, engineering_context: false });
  }
});

test("github: an issue and a merged pull request", async ({ page }) => {
  test.setTimeout(300_000);
  const github = await startPrivateStack();
  try {
    const { stack } = github;
    await installGhStub(stack.layout, [
      { number: 7, title: "Flows reach a merged pull request", state: "open", body: "Make the flows end at a merged PR." },
      { number: 8, title: "Fix #7", state: "closed", pull_request: { merged_at: "2026-11-02T09:30:00Z" } },
    ]);
    const added = await stack.archon.request({ cmd: "repo_add", path: await fixtureRepository(stack.layout), name: "e2e-repo" });
    expect(added.reply, "archond registers the fixture repository").not.toBe("error");
    await pauseQueue(stack);
    await pairBoth(page, stack, { aether: "operator", archon: "operator" }, "Design audit GitHub");
    await shoot(page, stack, "GitHub issue", "unread", `#/github/github.com/${OWNER_REPO}/7`, ".ui-state", GH_SYNC_MS);
    const imported = await stack.archon.request({ cmd: "import_issue", ref: { host: "github.com", repo: OWNER_REPO, number: 7, kind: "issue" } });
    const item = String((imported.data as { id?: unknown }).id);
    assert.match(item, /^qi_/, "archond imports the issue as an item");
    // archond caches GitHub reads without streaming a change, so the page reads them on a fresh load.
    await page.reload();
    await shoot(page, stack, "GitHub issue", "imported", `#/github/github.com/${OWNER_REPO}/7`, ".gh-fact", GH_SYNC_MS);
    const linked = await stack.archon.request({ cmd: "link_issue", item, ref: { host: "github.com", repo: OWNER_REPO, number: 8, kind: "pull_request" }, relation: "closes" });
    expect(linked.reply, "archond links the pull request to the item").toBe("ok");
    await until(() => stack.archon.request({ cmd: "item_links", item }),
      (reply) => JSON.stringify(reply.data).includes('"merged"'), "archond's sync to read the pull request", GH_SYNC_MS);
    await page.goto(`${stack.aether.origin}/#/github/github.com/${OWNER_REPO}/8`);
    await page.reload();
    await shoot(page, stack, "GitHub pull request", "merged", `#/github/github.com/${OWNER_REPO}/8`, ".gh-fact", GH_SYNC_MS);
    await shoot(page, stack, "GitHub overview", "loaded", "#/github", { ready: '[data-testid="gh-graph"]', readyMs: GH_SYNC_MS });
  } finally {
    await stopPrivateStack(github);
  }
});
