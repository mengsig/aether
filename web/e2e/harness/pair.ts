// Pairing a browser page with each daemon the way a person does: the host opens a code, the
// page's pairing screen claims it, and (for Aether) the host confirms the phrase.

import assert from "node:assert/strict";
import { expect, type Page } from "@playwright/test";
import { until } from "./process.ts";
import type { Stack } from "./stack.ts";

const PAIR_MS = 20_000;

/** The nav rail's entry for a daemon's connection; its `data-state` is core's connection state. */
export function connection(page: Page, daemon: "aether" | "archon") {
  return page.getByRole("list", { name: "Connections" }).getByRole("listitem")
    .filter({ has: page.locator(".ui-conn-kind", { hasText: new RegExp(`^${daemon}$`) }) });
}

export async function pairAether(page: Page, stack: Stack, deviceName = "E2E browser"): Promise<string> {
  const pairing = await stack.aether.pairing("observer");
  assert.equal(pairing.url, `${stack.aether.origin}/pair#code=${pairing.code}`, "the daemon prints the URL the app reads");
  await page.goto(`${stack.aether.origin}/pair#code=${pairing.code}`);
  const code = page.getByLabel("Pairing code");
  await expect(code).toHaveValue(pairing.code);
  await page.getByLabel("Name for this device").fill(deviceName);
  await page.getByRole("button", { name: "Pair" }).click();
  const claimed = await until(() => stack.aether.request({ cmd: "get_pairing", id: pairing.id }), (reply) => reply.data.state === "claimed",
    "the browser's claim on aetherd", PAIR_MS);
  const phrase = String((claimed.data.candidate as { phrase?: unknown } | undefined)?.phrase);
  await expect(page.getByText(phrase, { exact: true })).toBeVisible();
  const enrolled = await stack.aether.request({ cmd: "confirm_pairing", id: pairing.id, accept: true });
  assert.equal(enrolled.data.state, "enrolled");
  await expect(page.getByText("Paired with Aether")).toBeVisible({ timeout: PAIR_MS });
  return String((enrolled.data.device as { id?: unknown }).id);
}

export async function pairArchon(page: Page, stack: Stack, deviceName = "E2E browser"): Promise<void> {
  const { code } = await stack.archon.pairing();
  await page.goto(`${stack.aether.origin}/#/pair/archon`);
  await page.getByLabel("Pairing code").fill(code);
  await page.getByLabel("Name for this device").fill(deviceName);
  await stack.archon.claimSlot();
  await page.getByRole("button", { name: "Pair" }).click();
  await expect(page.getByText("Paired with Archon")).toBeVisible({ timeout: PAIR_MS });
}

/** Pairs `page` with both daemons and waits until both connections are ready. */
export async function pairBoth(page: Page, stack: Stack): Promise<void> {
  await pairAether(page, stack);
  await pairArchon(page, stack);
  await page.goto(`${stack.aether.origin}/#/fleet`);
  await expect(connection(page, "aether")).toHaveAttribute("data-state", "ready", { timeout: PAIR_MS });
  await expect(connection(page, "archon")).toHaveAttribute("data-state", "ready", { timeout: PAIR_MS });
}
