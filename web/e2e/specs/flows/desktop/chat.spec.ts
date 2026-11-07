// Flow 7: chatting in the desktop app window. A chat stays open for follow-ups, the composer keeps
// its draft across navigation and mode switches, the Archon orchestrator opens in its Calm mode
// read from aetherd's config, the / menu and the palette reach everything without typing it.

import assert from "node:assert/strict";
import { mkdir, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import type { Page } from "@playwright/test";
import { connection } from "../../../harness/pair.ts";
import { expect, test } from "../../../harness/fixtures.ts";
import type { Stack } from "../../../harness/stack.ts";
import { launchDesktop, type DesktopApp } from "./electron.ts";
import { aetherConn, route } from "./state.ts";

const FAUX_MODEL_REF = { provider: "mock", model: "mock-1" } as const;
const ORCHESTRATOR_ROLE = "archon-orchestrator";
const CALM = `version = 1
label = "Calm"
description = "Archon coordination and decisions without tool detail"
density = "compact"
show = ["user_text", "assistant_text", "delegation", "agent_message", "completion", "status", "question", "approval", "error"]

[scope]
role = "${ORCHESTRATOR_ROLE}"
`;
const SHOT = { width: 1440, height: 900 } as const;

test.describe.configure({ mode: "serial" });

let app: DesktopApp;
let conn: string;
let chat: string;
let orchestrator: string;
const pageErrors: string[] = [];

/** A chat as the desktop starts one: a task-less interactive root that waits for each message. */
async function createChat(stack: Stack, name: string, role: string): Promise<string> {
  const { reply, data } = await stack.aether.request({
    cmd: "create_agent", role, name, model: FAUX_MODEL_REF, start: true, config: { interactive: true },
  });
  assert.equal(reply, "agent", `create_agent answers with the agent: ${reply}`);
  assert.match(String(data.id), /^agt_/);
  return String(data.id);
}

async function agentState(stack: Stack, id: string): Promise<string> {
  const { reply, data } = await stack.aether.request({ cmd: "get_agent", id });
  assert.equal(reply, "agent", `get_agent answers with the agent: ${reply}`);
  return String((data.status as { state?: unknown }).state);
}

/** The window at 1440x900 in the light and the dark theme, then back to the system's. */
async function shoot(window: Page, name: string): Promise<void> {
  const info = test.info();
  await window.setViewportSize(SHOT);
  for (const scheme of ["light", "dark"] as const) {
    await window.emulateMedia({ colorScheme: scheme, reducedMotion: "reduce" });
    await expect(window.locator("html")).toHaveAttribute("data-theme", scheme);
    await window.screenshot({ path: info.outputPath(`${name}-${scheme}.png`) });
  }
  await window.emulateMedia({ colorScheme: null, reducedMotion: null });
}

const composer = (window: Page, name: string) => window.getByRole("combobox", { name: `Message ${name}` });
const modeRadio = (window: Page, label: string) => window.getByRole("radiogroup", { name: "View mode" }).getByRole("radio", { name: label });

test.beforeAll(async ({ stack }) => {
  const modes = path.join(stack.layout.aether.config, "modes");
  await mkdir(modes, { recursive: true });
  await writeFile(path.join(modes, "calm.toml"), CALM);
  stack.engine.setScript((request) => `Reply to: ${request.lastUserText.slice(-40)}\n\n\`\`\`js\nconsole.log("ok");\n\`\`\``);
  chat = await createChat(stack, "Desktop chat", "assistant");
  orchestrator = await createChat(stack, "Archon", ORCHESTRATOR_ROLE);
  await stack.aether.request({ cmd: "send_message", to: orchestrator, content: "What is running?" });
  conn = await aetherConn(stack);
  app = await launchDesktop({ aetherSocket: stack.layout.aether.socket, archonSocket: stack.layout.archon.socket });
  app.window.on("pageerror", (error) => pageErrors.push(error.message));
  await expect(connection(app.window, "aether")).toHaveAttribute("data-state", "ready", { timeout: 30_000 });
});

test.afterAll(async ({ stack }) => {
  await app?.close();
  await rm(path.join(stack.layout.aether.config, "modes"), { recursive: true, force: true });
});

test.afterEach(() => {
  expect(pageErrors.splice(0), "the app window threw no page error").toEqual([]);
});

test("UX-005/UX-006: the composer sends, the agent answers, and the chat stays open for a follow-up", async ({ stack }) => {
  await route(app.window, `#/agents/${conn}/${chat}`);
  const field = composer(app.window, "Desktop chat");
  await expect(field).toBeVisible();
  await field.fill("Show me a code example");
  await field.press("Enter");
  await expect(app.window.getByText("Reply to:", { exact: false }).first()).toBeVisible({ timeout: 30_000 });
  await expect(field, "a delivered message clears the draft").toHaveValue("");
  await expect.poll(() => agentState(stack, chat), { message: "the chat waits instead of completing" }).toBe("waiting");
  await field.fill("And one more");
  await field.press("Enter");
  await expect(app.window.getByText("And one more", { exact: false }).first()).toBeVisible({ timeout: 30_000 });
  expect(await agentState(stack, chat), "a follow-up never ends the chat").not.toBe("completed");
  await shoot(app.window, "chat-follow-up");
});

test("UX-007: an unsent draft survives navigating away and back", async () => {
  await route(app.window, `#/agents/${conn}/${chat}`);
  await composer(app.window, "Desktop chat").fill("A long instruction I have not sent yet");
  await route(app.window, "#/fleet");
  await expect(app.window.getByRole("region", { name: "Fleet" })).toBeVisible();
  await route(app.window, `#/agents/${conn}/${chat}`);
  await expect(composer(app.window, "Desktop chat")).toHaveValue("A long instruction I have not sent yet");
  await shoot(app.window, "draft-restored");
  await composer(app.window, "Desktop chat").fill("");
});

test("UX-010: the orchestrator opens in Calm; Standard and back by picker and by /calm keep the draft", async () => {
  await route(app.window, `#/agents/${conn}/${orchestrator}`);
  await expect(modeRadio(app.window, "Calm"), "the orchestrator defaults to its Calm mode").toBeChecked({ timeout: 30_000 });
  await expect(app.window.getByText("[inbox", { exact: false }), "no routing envelope reaches the reader").toHaveCount(0);
  await shoot(app.window, "orchestrator-calm");
  const field = composer(app.window, "Archon");
  await field.fill("draft kept across modes");
  await modeRadio(app.window, "Standard").click();
  await expect(modeRadio(app.window, "Standard")).toBeChecked();
  await expect(field, "switching modes keeps the draft").toHaveValue("draft kept across modes");
  await shoot(app.window, "orchestrator-standard");
  await field.fill("/calm");
  await field.press("Enter");
  await expect(modeRadio(app.window, "Calm"), "/calm toggles back").toBeChecked();
  await app.window.getByRole("button", { name: /Details/ }).click();
  await expect(app.window.getByRole("button", { name: /Details/ })).toHaveAttribute("aria-pressed", "true");
  await shoot(app.window, "orchestrator-calm-details");
  await app.window.getByRole("button", { name: /Details/ }).click();
});

test("the chosen mode is remembered per agent", async () => {
  await route(app.window, `#/agents/${conn}/${chat}`);
  await modeRadio(app.window, "Standard").click();
  await route(app.window, `#/agents/${conn}/${orchestrator}`);
  await expect(modeRadio(app.window, "Calm"), "the orchestrator keeps its own choice").toBeChecked();
  await route(app.window, `#/agents/${conn}/${chat}`);
  await expect(modeRadio(app.window, "Standard"), "the chat keeps Standard").toBeChecked();
});

test("the / menu lists app, view and Archon commands and runs one from the keyboard", async () => {
  await route(app.window, `#/agents/${conn}/${orchestrator}`);
  const field = composer(app.window, "Archon");
  await field.fill("/");
  const menu = app.window.getByRole("listbox", { name: "Commands" });
  await expect(menu).toBeVisible();
  for (const name of ["/settings", "/mode", "/answer"]) await expect(menu.getByRole("option", { name: new RegExp(`^${name}\\b`) })).toBeVisible();
  await shoot(app.window, "slash-menu");
  await field.fill("/mo");
  await field.press("Tab");
  await expect(field).toHaveValue(/^\/mode /);
  await expect(menu.getByRole("option", { name: /Focus/ })).toBeVisible();
  await menu.getByRole("option", { name: /Focus/ }).click();
  await field.press("Enter");
  await expect(modeRadio(app.window, "Focus")).toBeChecked();
});

test("UX-046: the palette finds an agent by name and opens its chat", async () => {
  await route(app.window, "#/fleet");
  await app.window.keyboard.press("Control+k");
  const search = app.window.getByRole("combobox", { name: "Search screens, agents and work items" });
  await search.fill("Desktop ch");
  const option = app.window.getByRole("option", { name: /Desktop chat/ });
  await expect(option).toBeVisible();
  await shoot(app.window, "palette-agent");
  await search.press("Enter");
  await expect(composer(app.window, "Desktop chat"), "Enter opens the agent's chat").toBeVisible();
});
