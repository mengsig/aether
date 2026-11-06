// Flow 6b: the Archon controls the desktop app window shows reach archond through main's bridge:
// Message Archon, answering a question, and an initiative's detail. Each is checked on archond.
// With E2E_DESKTOP_PACKAGED set it drives that packaged build instead of test/e2e/main.ts, and with
// E2E_SHOTS set each step is shot at 1440x900 into that directory.

import assert from "node:assert/strict";
import path from "node:path";
import type { Stack } from "../../../harness/stack.ts";
import { connection } from "../../../harness/pair.ts";
import { expect, test, type Page } from "../../../harness/fixtures.ts";
import { fixtureRepository } from "../gh-stub.ts";
import { startPrivateStack, stopPrivateStack, type PrivateStack } from "../private.ts";
import { launchDesktop } from "./electron.ts";
import { launchPackaged, packagedBinary } from "./packaged.ts";
import { route, setPaused } from "./state.ts";

const SHOTS = process.env.E2E_SHOTS;
const VIEWPORT = { width: 1440, height: 900 } as const;
const MESSAGE = "Desktop e2e: hello from the app window";
const QUESTION = "Desktop e2e: which summary style?";
const INITIATIVE = "Desktop e2e initiative";
const CONTRACT = "Both members share one README section.";

test.describe.configure({ mode: "serial" });

let running: PrivateStack | undefined;
let app: { readonly window: Page; close(): Promise<void> } | undefined;
const pageErrors: string[] = [];

function stack(): Stack {
  assert.ok(running !== undefined, "the private stack is up");
  return running.stack;
}

function window(): Page {
  assert.ok(app !== undefined, "the desktop app is up");
  return app.window;
}

async function shoot(name: string): Promise<void> {
  if (SHOTS === undefined || SHOTS === "") return;
  assert.ok(path.isAbsolute(SHOTS), `E2E_SHOTS names an absolute directory, not ${SHOTS}`);
  await window().screenshot({ path: path.join(SHOTS, `${name}.png`) });
}

test.beforeAll(async () => {
  test.setTimeout(180_000);
  running = await startPrivateStack();
  // Nothing the flow queues may start: its items only need to exist.
  await setPaused(stack(), true);
  const { aether, archon } = stack().layout;
  const packaged = packagedBinary();
  app = packaged === undefined ? await launchDesktop({ aetherSocket: aether.socket, archonSocket: archon.socket })
    : await launchPackaged(packaged, { aether: aether.socket, archon: archon.socket });
  app.window.on("pageerror", (error) => pageErrors.push(error.message));
  await app.window.setViewportSize(VIEWPORT);
  await expect(connection(app.window, "archon")).toHaveAttribute("data-state", "ready", { timeout: 30_000 });
});

test.afterAll(async () => {
  try {
    await app?.close();
  } finally {
    await stopPrivateStack(running);
  }
});

test.afterEach(() => {
  expect(pageErrors.splice(0), "the app window threw no page error").toEqual([]);
});

test("Message Archon from the app window lands in archond's feed as the operator's line", async () => {
  const page = window();
  await route(page, "#/archon/queue");
  await page.getByRole("button", { name: "Message Archon" }).click();
  const dialog = page.getByRole("dialog", { name: "Message Archon" });
  await dialog.getByLabel("Message").fill(MESSAGE);
  await dialog.getByRole("button", { name: "Send" }).click();
  const outcome = dialog.getByRole("status").or(dialog.getByRole("alert"));
  await expect(outcome).toBeVisible();
  await shoot("message-archon");
  await expect(dialog.getByRole("status")).toHaveText("Sent to the orchestrator.");
  await expect.poll(async () => {
    const { data } = await stack().archon.request({ cmd: "feed_history", before: null, limit: 50, verbose: false });
    const feed = data as unknown as Array<{ entry: { entry: string; text?: string } }>;
    return feed.some((item) => item.entry.entry === "operator" && item.entry.text === MESSAGE);
  }, { message: "archond's feed holds the app's line" }).toBe(true);
});

test("answering a question in the app window answers it in archond", async () => {
  const asked = await stack().archon.request({ cmd: "ask_operator", text: QUESTION, item: null, options: ["Concise", "Detailed"] });
  assert.equal(asked.reply, "question", `archond asks the question: ${JSON.stringify(asked)}`);
  const id = String(asked.data.id);
  const page = window();
  await route(page, "#/archon/queue");
  const card = page.getByRole("region", { name: "Open questions" }).getByRole("listitem").filter({ hasText: QUESTION });
  await card.getByRole("button", { name: "Concise" }).click();
  await expect.poll(async () => (await card.count()) === 0 || (await card.getByRole("alert").count()) > 0,
    { message: "the answer settles: the card leaves or says why not" }).toBe(true);
  await shoot("answer-question");
  await expect.poll(async () => {
    const { data } = await stack().archon.request({ cmd: "list_questions", open_only: false });
    return (data as unknown as Array<{ id: string; answer?: string | null }>).find((question) => question.id === id)?.answer ?? null;
  }, { message: "archond records the app's answer" }).toBe("Concise");
  await expect(page.getByRole("region", { name: "Open questions" }).getByText(QUESTION)).toHaveCount(0);
});

test("an initiative's detail opens in the app window", async () => {
  const added = await stack().archon.request({ cmd: "repo_add", path: await fixtureRepository(stack().layout), name: "e2e-repo" });
  assert.notEqual(added.reply, "error", `archond registers the fixture repository: ${JSON.stringify(added)}`);
  const members = ["first", "second"].map((title) => ({ repo: "e2e-repo", title: `Initiative ${title} member`, brief: `The ${title} member.` }));
  const made = await stack().archon.request({ cmd: "enqueue_initiative", title: INITIATIVE, contract: CONTRACT, members });
  assert.equal(made.reply, "initiative", `archond creates the initiative: ${JSON.stringify(made)}`);
  const id = String((made.data.initiative as { id?: unknown } | undefined)?.id ?? made.data.id);
  assert.ok(id !== "" && id !== "undefined", `the initiative has an id: ${JSON.stringify(made.data)}`);
  const page = window();
  await route(page, `#/archon/initiatives/${id}`);
  await expect(page.locator(".initiative-title").or(page.getByRole("alert"))).toBeVisible({ timeout: 30_000 });
  await shoot("initiative-detail");
  await expect(page.locator(".initiative-title")).toHaveText(INITIATIVE);
  await expect(page.getByRole("region", { name: "Contract" })).toContainText(CONTRACT);
  await expect(page.getByRole("region", { name: "Members" }).locator(".dag-count")).toHaveText("2");
  await expect(connection(page, "archon"), "the read kept the link up").toHaveAttribute("data-state", "ready");
});
