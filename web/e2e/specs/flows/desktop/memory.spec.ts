import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { readdir } from "node:fs/promises";
import path from "node:path";
import { promisify } from "node:util";
import { expect, test, type Page } from "../../../harness/fixtures.ts";
import { connection } from "../../../harness/pair.ts";
import { startPrivateStack, stopPrivateStack, type PrivateStack } from "../private.ts";
import { launchPackaged, packagedBinary } from "./packaged.ts";
import { aetherConn, route } from "./state.ts";

let running: PrivateStack | undefined;
let app: { window: Page; waylandDisplay: string; close(): Promise<void> } | undefined;
let conn = "";
let agent = "";

function page(): Page {
  assert.ok(app, "the packaged desktop is running");
  return app.window;
}

async function screenshot(name: string): Promise<void> {
  const shots = process.env.E2E_SHOTS;
  if (!shots) return;
  for (const theme of ["light", "dark"] as const) {
    await page().emulateMedia({ colorScheme: theme });
    await page().screenshot({ path: path.join(shots, `${name}-${theme}.png`) });
  }
}

interface SwayNode { id: number; name: string | null; focused: boolean; nodes: SwayNode[]; floating_nodes: SwayNode[] }

function findDialog(node: SwayNode): SwayNode | undefined {
  if (node.name === "Import memories") return node;
  for (const child of [...node.nodes, ...node.floating_nodes]) {
    const found = findDialog(child);
    if (found) return found;
  }
  return undefined;
}

async function confirmNativeImport(display: string): Promise<void> {
  const runtime = path.dirname(display);
  const socket = (await readdir(runtime)).find((name) => /^sway-ipc\..*\.sock$/.test(name));
  assert.ok(socket, "the private compositor has an IPC socket");
  const sway = async (...args: string[]) => (await promisify(execFile)("swaymsg", ["-s", path.join(runtime, socket), ...args])).stdout;
  let dialog: SwayNode | undefined;
  await expect.poll(async () => {
    dialog = findDialog(JSON.parse(await sway("-t", "get_tree")) as SwayNode);
    return dialog !== undefined;
  }, { message: "the native import confirmation appears", timeout: 10_000 }).toBe(true);
  assert.ok(dialog && Number.isSafeInteger(dialog.id), "the native dialog has a compositor id");
  const result = JSON.parse(await sway(`[con_id=${dialog.id}]`, "focus")) as Array<{ success: boolean }>;
  assert.ok(result.length > 0 && result.every((item) => item.success), "the native dialog receives focus");
  await promisify(execFile)("wtype", ["-k", "Tab", "-k", "Return"], { env: { ...process.env, WAYLAND_DISPLAY: display, XDG_RUNTIME_DIR: runtime } });
}

test.describe.configure({ mode: "serial" });
test.beforeAll(async () => {
  test.setTimeout(180_000);
  const binary = packagedBinary();
  assert.ok(binary, "E2E_DESKTOP_PACKAGED names the built application");
  running = await startPrivateStack();
  const stack = running.stack;
  const created = await stack.aether.request({ cmd: "create_agent", name: "Memory workspace", role: "worker", model: { provider: "mock", model: "mock-1" }, scope: { workspace_root: stack.layout.aether.work } });
  agent = String(created.data.id);
  assert.match(agent, /^agt_/);
  conn = await aetherConn(stack);
  app = await launchPackaged(binary, { aether: stack.layout.aether.socket, archon: stack.layout.archon.socket });
  await page().setViewportSize({ width: 1440, height: 900 });
  await expect(connection(page(), "aether")).toHaveAttribute("data-state", "ready", { timeout: 30_000 });
  await route(page(), `#/memory/${encodeURIComponent(conn)}`);
});

test.afterAll(async () => {
  try { await app?.close(); } finally { await stopPrivateStack(running); }
});

test("memory review and maintenance remain available after navigation", async () => {
  await expect(page().getByRole("region", { name: "Proposals", exact: true })).toContainText("No proposals to review");
  await expect(page().getByRole("region", { name: "Retrieval explain" }).getByRole("button", { name: "Explain" })).toBeVisible();
  const maintenance = page().getByRole("region", { name: "Maintenance" });
  await maintenance.getByRole("button", { name: "Plan consolidation" }).click();
  await expect(maintenance).toContainText("Nothing to merge.");
  await expect(maintenance.getByRole("button", { name: "Export memories" })).toBeVisible();
  await expect(maintenance.getByLabel("Export file (.jsonl)")).toBeVisible();
  await route(page(), "#/inbox");
  await route(page(), `#/memory/${encodeURIComponent(conn)}`);
  await expect(page().getByRole("region", { name: "Maintenance" }).getByRole("button", { name: "Plan consolidation" })).toBeVisible();
  await screenshot("ux-017-maintenance");
});

test("repository creation, filtering and store counts agree", async () => {
  assert.ok(running, "the private stack is running");
  await page().getByRole("button", { name: "Create memory" }).click();
  const sheet = page().getByRole("dialog", { name: "Create memory" });
  await sheet.getByLabel("Memory text").fill("The private repository keeps its release notes here.");
  await sheet.getByRole("tab", { name: "Repository" }).click();
  await sheet.getByLabel("Repository root").selectOption(running.stack.layout.aether.work);
  await sheet.getByRole("button", { name: "Save memory" }).click();
  await expect(sheet).toHaveCount(0);
  const count = page().getByRole("region", { name: "Store" }).locator("dt").filter({ hasText: /^Memories$/ }).locator("..").locator("dd");
  await expect(count).toHaveText("1");
  await page().getByRole("tab", { name: "Repository", exact: true }).click();
  const repository = page().getByLabel("Repository filter");
  await expect(repository.locator("option")).toHaveCount(3);
  await repository.selectOption({ index: 1 });
  await expect(page().getByText("The private repository keeps its release notes here.")).toBeVisible();
  await expect(connection(page(), "aether")).toHaveAttribute("data-state", "ready");
  await screenshot("ux-018-021-repository");
  await route(page(), "#/inbox");
  await route(page(), `#/memory/${encodeURIComponent(conn)}`);
  await page().getByRole("button", { name: "Create memory" }).click();
  await sheet.getByLabel("Memory text").fill("The agent keeps this separate fact.");
  await sheet.getByRole("tab", { name: "Agent" }).click();
  await sheet.getByRole("combobox", { name: "Agent", exact: true }).selectOption(agent);
  await sheet.getByRole("button", { name: "Save memory" }).click();
  await expect(sheet).toHaveCount(0);
  await expect(count).toHaveText("2");
  await screenshot("ux-021-counts");
});

test("a refused memory shows its reason and can be corrected or discarded", async () => {
  await page().getByRole("button", { name: "Create memory" }).click();
  const sheet = page().getByRole("dialog", { name: "Create memory" });
  await sheet.getByLabel("Memory text").fill("A durable fact that can be corrected.");
  await sheet.getByLabel("Tags").fill("x".repeat(65));
  await sheet.getByRole("button", { name: "Save memory" }).click();
  await expect(sheet.getByRole("alert")).toContainText("tags");
  await expect(connection(page(), "aether")).toHaveAttribute("data-state", "ready");
  await screenshot("ux-019-refused");
  await sheet.getByRole("button", { name: "Discard" }).click();
  await page().getByRole("button", { name: "Create memory" }).click();
  await sheet.getByLabel("Memory text").fill("A durable fact that can be corrected.");
  await sheet.getByLabel("Tags").fill("release");
  await sheet.getByRole("button", { name: "Save memory" }).click();
  await expect(sheet).toHaveCount(0);
  await route(page(), "#/inbox");
  await expect(page().getByRole("region", { name: "Recovery review" })).toHaveCount(0);
});

test("a reviewed import reaches the store after native confirmation", async () => {
  const donor = await startPrivateStack();
  let artifact: string;
  try {
    await donor.stack.aether.request({ cmd: "create_memory", spec: { text: "A memory imported from another private store.", kind: "fact", scope: { scope: "global" } } });
    const exported = await donor.stack.aether.request({ cmd: "memory_export" });
    assert.equal(exported.reply, "memory_export");
    artifact = String(exported.data.artifact);
  } finally {
    await stopPrivateStack(donor);
  }
  assert.ok(app, "the packaged desktop is running");
  await route(page(), `#/memory/${encodeURIComponent(conn)}`);
  const maintenance = page().getByRole("region", { name: "Maintenance" });
  await maintenance.getByLabel("Export file (.jsonl)").setInputFiles({ name: "memories.jsonl", mimeType: "application/jsonl", buffer: Buffer.from(artifact) });
  await maintenance.getByRole("button", { name: "Dry run import" }).click();
  await expect(maintenance).toContainText("Would insert");
  await maintenance.getByRole("button", { name: "Import", exact: true }).click();
  await page().getByRole("dialog", { name: "Import memories?" }).getByRole("button", { name: "Import memories", exact: true }).click();
  await confirmNativeImport(app.waylandDisplay);
  await expect(maintenance).toContainText("Imported", { timeout: 15_000 });
  await expect(page().getByText("A memory imported from another private store.")).toBeVisible();
  await screenshot("ux-017-import");
});
