// What the Noesis specs share: the commands a page sends, Noesis status and settings over the
// host socket, the files Noesis would make, and the entity screens "Learn from this" sits on.

import assert from "node:assert/strict";
import { readdir } from "node:fs/promises";
import { expect, type Page } from "@playwright/test";
import { connection } from "../../harness/pair.ts";
import type { Stack } from "../../harness/stack.ts";

/** The `noesis_*` commands that only read; every other one changes something. */
export const NOESIS_READS: ReadonlySet<string> = new Set(["noesis_status", "noesis_get_session", "noesis_list_sessions", "noesis_graph",
  "noesis_jobs", "noesis_learner", "noesis_learner_evidence"]);
/** What starts Noesis work: a session, an extraction job, an imported source. */
export const STARTERS: readonly string[] = ["noesis_create_session", "noesis_extract", "noesis_import_source"];
export const LEARN_FROM_THIS = "Learn from this";

export interface SentCommands {
  /** The `cmd` of every command frame the page sent, in order. */
  readonly commands: string[];
  /** Frames that were not JSON objects; the transports send none. */
  readonly unparsed: string[];
}

/** Records every frame the page sends on any WebSocket; call it before the page opens one. */
export function recordCommands(page: Page): SentCommands {
  const sent: SentCommands = { commands: [], unparsed: [] };
  page.on("websocket", (socket) => {
    socket.on("framesent", ({ payload }) => {
      const text = typeof payload === "string" ? payload : payload.toString("utf8");
      let frame: unknown;
      try {
        frame = JSON.parse(text);
      } catch {
        sent.unparsed.push(text.slice(0, 80));
        return;
      }
      const cmd = (frame as { cmd?: unknown } | null)?.cmd;
      if (typeof cmd === "string") sent.commands.push(cmd);
      else sent.unparsed.push(text.slice(0, 80));
    });
  });
  return sent;
}

export const noesisCommands = (commands: readonly string[]): string[] => commands.filter((cmd) => cmd.startsWith("noesis_"));
export const noesisChanges = (commands: readonly string[]): string[] => noesisCommands(commands).filter((cmd) => !NOESIS_READS.has(cmd));

export interface NoesisStatus {
  readonly enabled: boolean;
  readonly jobs_running: number;
  readonly settings: { readonly enabled: boolean; readonly engineering_context: boolean };
  readonly effective: { readonly enabled: boolean; readonly engineering_context: boolean };
}

function asStatus(reply: { reply: string; data: Record<string, unknown> }, what: string): NoesisStatus {
  assert.equal(reply.reply, "noesis_status", `${what} answers noesis_status: ${reply.reply}`);
  const status = reply.data as unknown as NoesisStatus;
  assert.equal(typeof status.enabled, "boolean", `${what} carries enabled`);
  assert.equal(typeof status.settings?.engineering_context, "boolean", `${what} carries the stored settings`);
  assert.equal(typeof status.effective?.engineering_context, "boolean", `${what} carries the effective settings`);
  return status;
}

export async function noesisStatus(stack: Stack): Promise<NoesisStatus> {
  return asStatus(await stack.aether.request({ cmd: "noesis_status" }), "noesis_status");
}

/** `noesis_configure` as the host, over the socket; returns the status it answers with. */
export async function configureNoesis(stack: Stack, patch: { enabled?: boolean; engineering_context?: boolean }): Promise<NoesisStatus> {
  assert.ok(Object.keys(patch).length > 0, "a configure changes something");
  const status = asStatus(await stack.aether.request({ cmd: "noesis_configure", patch }), "noesis_configure");
  if (patch.enabled !== undefined) assert.equal(status.enabled, patch.enabled, "the service follows the master switch");
  return status;
}

/** Every file in Aether's data dir whose name starts with `noesis` (the db and its journals). */
export async function noesisFiles(stack: Stack): Promise<string[]> {
  const names = await readdir(stack.layout.aether.data);
  assert.ok(names.length > 0, `aetherd keeps its own state in ${stack.layout.aether.data}`);
  return names.filter((name) => name.startsWith("noesis"));
}

/** Reloads the page, so it reads `noesis_status` afresh, and waits until both connections are ready. */
export async function reconnect(page: Page): Promise<void> {
  await page.reload();
  await expect(connection(page, "aether")).toHaveAttribute("data-state", "ready", { timeout: 20_000 });
  await expect(connection(page, "archon")).toHaveAttribute("data-state", "ready", { timeout: 20_000 });
}

/** Opens an agent's screen from the fleet; returns the Aether connection id its route names. */
export async function openAgent(page: Page, stack: Stack, name: string): Promise<string> {
  await page.goto(`${stack.aether.origin}/#/fleet`);
  await page.getByRole("region", { name: "Fleet" }).getByRole("link", { name, exact: true }).click();
  await expect(page.locator(".agent-name")).toHaveText(name);
  const match = /#\/agents\/([^/]+)\/(agt_[^/?]+)/.exec(page.url());
  assert.ok(match?.[1] !== undefined, `the agent screen's route names its connection: ${page.url()}`);
  return decodeURIComponent(match[1]);
}

export async function openItem(page: Page, stack: Stack, title: string): Promise<void> {
  await page.goto(`${stack.aether.origin}/#/archon/queue`);
  await page.getByRole("region", { name: "Queue" }).getByRole("link", { name: title, exact: true }).click();
  await expect(page.locator("#item-title")).toHaveText(title);
}

/** The labels of an entity header's action menu, or [] when it offers no menu at all. */
export async function entityActions(page: Page, header: string): Promise<string[]> {
  const trigger = page.locator(header).getByRole("button", { name: "Actions", exact: true });
  if (await trigger.count() === 0) return [];
  await trigger.click();
  const menu = page.getByRole("menu", { name: "Actions" });
  await expect(menu, "the action menu opens").toBeVisible();
  const labels = (await menu.getByRole("menuitem").allTextContents()).map((label) => label.trim());
  await page.keyboard.press("Escape");
  await expect(menu, "the action menu closes").toHaveCount(0);
  return labels;
}
