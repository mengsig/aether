// Flow 6: the desktop app window on this machine's own sockets. Through its UI, and so through
// window.desktop and main's bridge, it reads the fleet and the queue and changes both daemons;
// each change is checked on the daemon, with who archond says made it.

import { createAgent, enqueueItem, itemTitles, listAgents, queuePaused } from "../../../harness/agents.ts";
import { connection, everyQueueRow } from "../../../harness/pair.ts";
import { expect, test } from "../../../harness/fixtures.ts";
import { launchDesktop, type DesktopApp } from "./electron.ts";
import { aetherConn, itemState, lastTransitionTo, route, setPaused, type TransitionActor } from "./state.ts";

/** The window's own changes pass through the bridge on a hello core writes and H3 holds to `archon-web`. */
const PAGE_ARCHON_ACTOR: TransitionActor = "web";
/**
 * A natively confirmed privileged change runs on main's control link, which keeps the operator's
 * `archon-cli` hello: only that client may, for one, lift the operator's hard pause.
 */
const OPERATOR_ARCHON_ACTOR: TransitionActor = "cli";

const AGENT = "Desktop bridge worker";
const RENAMED = "Renamed in the desktop app";
const ITEM = "Desktop bridge item";

test.describe.configure({ mode: "serial" });

let app: DesktopApp;
let pausedBefore: boolean;
const pageErrors: string[] = [];

test.beforeAll(async ({ stack }) => {
  pausedBefore = await queuePaused(stack);
  await setPaused(stack, false);
  await createAgent(stack, AGENT, "Wait to be renamed from the desktop app.", false);
  app = await launchDesktop({ aetherSocket: stack.layout.aether.socket, archonSocket: stack.layout.archon.socket });
  app.window.on("pageerror", (error) => pageErrors.push(error.message));
  await expect(connection(app.window, "aether")).toHaveAttribute("data-state", "ready", { timeout: 30_000 });
  await expect(connection(app.window, "archon")).toHaveAttribute("data-state", "ready", { timeout: 30_000 });
});

test.afterAll(async ({ stack }) => {
  await app?.close();
  const leftover = (await stack.archon.request({ cmd: "list_item_summaries" })).data as unknown as Array<{ id: string; title: string; state: { state: string } }>;
  for (const item of leftover.filter((row) => row.title === ITEM && row.state.state === "queued")) {
    await stack.archon.request({ cmd: "cancel_item", item: item.id, reason: "e2e cleanup" });
  }
  await setPaused(stack, pausedBefore);
});

test.afterEach(() => {
  expect(pageErrors.splice(0), "the app window threw no page error").toEqual([]);
});

test("the app window reads the fleet and the queue the daemons hold", async ({ stack }) => {
  const window = app.window;
  await route(window, "#/fleet");
  const fleet = window.getByRole("region", { name: "Fleet" });
  const agents = await listAgents(stack);
  // Earlier specs on this stack may have named two agents alike, so links are counted by name.
  const names = new Set(agents.map((agent) => agent.name));
  for (const name of names) await expect(fleet.getByRole("link", { name, exact: true })).toHaveCount(agents.filter((agent) => agent.name === name).length);
  await expect(fleet.locator(".fleet-count")).toHaveText(`${agents.length} of ${agents.length} agents`);

  await route(window, "#/archon/queue");
  await expect(window.locator(".queue-pause strong")).toHaveText("Running");
  await expect(await everyQueueRow(window)).toHaveCount((await itemTitles(stack)).length);
  const reads = (await app.calls()).filter((call) => call.method === "request" || call.method === "subscribe");
  expect(reads.length, "the page read the daemons through the bridge").toBeGreaterThan(0);
});

test("renaming an agent in the app window renames it in aetherd", async ({ stack }) => {
  const window = app.window;
  const id = (await listAgents(stack)).find((agent) => agent.name === AGENT)?.id;
  expect(id, "the agent to rename exists").toBeDefined();
  await route(window, `#/controls/${encodeURIComponent(await aetherConn(stack))}/${id}`);
  const controls = window.getByRole("region", { name: `Controls for ${AGENT}` });
  await controls.getByRole("tab", { name: "Model & limits" }).click();
  await controls.getByLabel("New name").fill(RENAMED);
  await controls.getByRole("button", { name: "Rename", exact: true }).click();
  await expect.poll(async () => (await listAgents(stack)).find((agent) => agent.id === id)?.name, { message: "aetherd holds the new name" }).toBe(RENAMED);
  await expect(window.getByRole("heading", { name: RENAMED })).toBeVisible();
});

test("pausing the queue and cancelling an item in the app window change archond, attributed to the page's hello", async ({ stack }) => {
  const window = app.window;
  await route(window, "#/archon/queue");
  await window.getByRole("button", { name: "Pause new work" }).click();
  await expect(window.locator(".queue-pause strong")).toHaveText("Paused");
  await expect.poll(() => queuePaused(stack), { message: "the window's pause reached archond" }).toBe(true);

  const item = await enqueueItem(stack, ITEM);
  await window.getByRole("region", { name: "Queue" }).getByRole("link", { name: ITEM, exact: true }).click();
  await expect(window.locator("#item-title")).toHaveText(ITEM);
  await window.getByRole("group", { name: "Item actions" }).getByRole("button", { name: "Cancel", exact: true }).click();
  await window.getByRole("dialog").getByRole("button", { name: "Cancel item" }).click();
  await expect.poll(() => itemState(stack, item), { message: "the window's cancel reached archond" }).toBe("cancelled");
  expect((await lastTransitionTo(stack, item, "cancelled")).by, "archond attributes the page's cancel to its archon-web hello").toBe(PAGE_ARCHON_ACTOR);
});

test("a privileged Archon change goes over main's control link, and archond attributes it to the operator", async ({ stack }) => {
  const item = (await stack.archon.request({ cmd: "list_item_summaries" })).data as unknown as Array<{ id: string; title: string }>;
  const id = item.find((row) => row.title === ITEM)?.id;
  expect(id, "the cancelled item from the previous test").toBeDefined();
  const conn = await archonBridgeConn(app);
  await app.answer(true);
  const outcome = await app.window.evaluate(async ({ conn: local, item: target }) => {
    const bridge = (window as unknown as { desktop: { privileged(action: unknown): Promise<unknown> } }).desktop;
    return bridge.privileged({ kind: "archon.operator_change", conn: local, request: "unblock", item: target }).then(() => "ok", (error: Error) => error.message);
  }, { conn, item: id as string });
  expect(outcome, "main confirmed and sent the unblock").toBe("ok");
  const [shown] = await app.confirmations();
  expect(`${shown?.title}\n${shown?.message}\n${shown?.detail ?? ""}`, "the native confirmation names the item").toContain(id as string);
  await expect.poll(() => itemState(stack, id as string), { message: "the retry reached archond" }).toBe("queued");
  expect((await lastTransitionTo(stack, id as string, "queued")).by, "archond attributes the confirmed retry to the operator's control link").toBe(OPERATOR_ARCHON_ACTOR);
});

/** The bridge connection the page opened to archond: the one it greeted with core's archon-web hello. */
async function archonBridgeConn(desktop: DesktopApp): Promise<string> {
  const hellos = (await desktop.calls()).filter((call) => call.method === "request"
    && (call.args[1] as { cmd?: unknown; client?: unknown } | undefined)?.cmd === "hello"
    && (call.args[1] as { client?: unknown }).client === "archon-web");
  const conn = hellos.at(-1)?.args[0];
  expect(typeof conn === "string" && conn.startsWith("local:"), `the page greeted archond through the bridge: ${JSON.stringify(hellos)}`).toBe(true);
  return conn as string;
}
