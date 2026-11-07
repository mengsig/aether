// Flow 1, Aether alone: no archond and no /archon mount. The app pairs, shows the fleet as aetherd
// holds it, and treats Archon as absent rather than broken: no Archon connection, no error.

import { existsSync } from "node:fs";
import { createAgent, listAgents } from "../../harness/agents.ts";
import { connection, fleetLink, pairAether } from "../../harness/pair.ts";
import { expect, test } from "../../harness/fixtures.ts";
import { startPrivateStack, stopPrivateStack, type PrivateStack } from "./private.ts";

let only: PrivateStack | undefined;

test.beforeAll(async () => {
  only = await startPrivateStack({ archon: false });
});

test.afterAll(async () => {
  await stopPrivateStack(only);
});

test("with Aether alone the app works and shows Archon as absent, without errors", async ({ page }, info) => {
  const { stack } = only as PrivateStack;
  expect(stack.archon.pid, "no archond was started").toBeUndefined();
  expect(existsSync(stack.layout.archon.socket), "no archond socket exists").toBe(false);
  const remote = await stack.aether.request({ cmd: "get_remote" });
  expect(remote.data.settings, "aetherd's listener is on").toMatchObject({ enabled: true });
  expect((remote.data.settings as { mounts?: unknown[] }).mounts ?? [], "aetherd mounts nothing").toEqual([]);
  const health = await page.request.get(`${stack.aether.origin}/api/health`);
  expect(health.ok()).toBe(true);
  expect((await health.json() as { mounts?: unknown[] }).mounts, "the listener's health names no mount").toEqual([]);

  await createAgent(stack, "Alone on Aether", "Wait without Archon.", false);
  const archonRequests: string[] = [];
  page.on("websocket", (socket) => { if (new URL(socket.url()).pathname.startsWith("/archon")) archonRequests.push(socket.url()); });
  page.on("request", (request) => { if (new URL(request.url()).pathname.startsWith("/archon")) archonRequests.push(request.url()); });
  const device = await pairAether(page, stack, "Aether only");
  await page.goto(`${stack.aether.origin}/#/fleet`);
  await expect(connection(page, "aether")).toHaveAttribute("data-state", "ready");
  await expect(connection(page, "archon"), "no Archon connection is listed").toHaveCount(0);
  const fleet = page.getByRole("region", { name: "Fleet" });
  await fleetLink(page, "Alone on Aether");
  const agents = await listAgents(stack);
  await expect(fleet.locator(".fleet-count")).toHaveText(`${agents.length} of ${agents.length} agents`);
  await page.screenshot({ path: info.outputPath("fleet-aether-only.png") });

  await page.goto(`${stack.aether.origin}/#/archon/queue`);
  await expect(page.getByRole("region", { name: "Archon not connected" }), "the queue says Archon is absent").toBeVisible();
  await expect(page.locator('.ui-state[data-kind="error"]'), "the queue shows no error state").toHaveCount(0);
  await expect(page.getByRole("alert"), "the Archon screen raises no alert").toHaveCount(0);
  await page.screenshot({ path: info.outputPath("queue-aether-only.png") });

  expect(archonRequests, "the page never asked for Archon").toEqual([]);
  const devices = await stack.aether.request({ cmd: "list_devices" });
  expect(JSON.stringify(devices.data), "aetherd enrolled the page").toContain(device);
});
