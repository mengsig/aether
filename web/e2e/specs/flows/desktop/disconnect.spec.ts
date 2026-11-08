import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import path from "node:path";
import { expect, test, type Page } from "@playwright/test";
import { connection } from "../../../harness/pair.ts";
import { startPrivateStack, stopPrivateStack, type PrivateStack } from "../private.ts";
import { launchPackaged, packagedBinary, type PackagedApp } from "./packaged.ts";
import { aetherConn, route } from "./state.ts";

const binary = packagedBinary();
test.skip(binary === undefined, "this journey requires E2E_DESKTOP_PACKAGED");

let running: PrivateStack | undefined;
let app: PackagedApp | undefined;
const errors: string[] = [];

async function screenshots(page: Page, name: string): Promise<void> {
  const dir = process.env.E2E_SHOTS ?? test.info().outputDir;
  assert.ok(path.isAbsolute(dir), "the screenshot directory is absolute");
  await mkdir(dir, { recursive: true });
  for (const theme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
    await expect(page.locator("html")).toHaveAttribute("data-theme", theme);
    await page.screenshot({ path: path.join(dir, `${name}-${theme}.png`) });
  }
}

test.beforeAll(async () => {
  test.setTimeout(180_000);
  assert.ok(binary, "the packaged binary is configured");
  running = await startPrivateStack();
  const { stack } = running;
  const created = await stack.aether.request({
    cmd: "create_agent", name: "Disconnect journey", role: "assistant", model: { provider: "mock", model: "mock-1" },
    start: true, config: { interactive: true },
  });
  assert.equal(created.reply, "agent");
  const agent = String(created.data.id);
  assert.match(agent, /^agt_/);
  const conn = await aetherConn(stack);
  app = await launchPackaged(binary, { aether: stack.layout.aether.socket, archon: stack.layout.archon.socket });
  app.window.on("pageerror", (error) => errors.push(error.message));
  await app.window.setViewportSize({ width: 1440, height: 900 });
  await expect(connection(app.window, "aether")).toHaveAttribute("data-state", "ready", { timeout: 30_000 });
  await route(app.window, `#/agents/${conn}/${agent}`);
  await expect(app.window.getByRole("combobox", { name: "Message Disconnect journey" })).toBeVisible();
  await expect(app.window.getByTestId("conv-connection")).toHaveCount(0);
});

test.afterAll(async () => {
  try { await app?.close(); } finally { await stopPrivateStack(running); }
});

test("the packaged conversation detects an idle daemon shutdown and reconnects", async () => {
  test.setTimeout(120_000);
  assert.ok(running && app, "the private stack and packaged app are running");
  const { stack } = running;
  const { window } = app;
  const banner = window.getByTestId("conv-connection");
  const stopped = stack.aether.stop();
  await Promise.all([
    stopped,
    expect(banner, "the idle conversation reports the external shutdown within five seconds").toBeVisible({ timeout: 5_000 }),
  ]);
  await expect(banner).toContainText(/lost|disconnected/i);
  await screenshots(window, "daemon-disconnected");
  await stack.aether.start();
  await expect(connection(window, "aether")).toHaveAttribute("data-state", "ready", { timeout: 30_000 });
  await expect(banner, "the conversation clears its disconnected banner after reconnecting").toHaveCount(0, { timeout: 30_000 });
  await expect(window.getByRole("combobox", { name: "Message Disconnect journey" })).toBeVisible();
  await screenshots(window, "daemon-reconnected");
  expect(errors, "the packaged window threw no page errors").toEqual([]);
});
