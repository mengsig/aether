// Journey 5, memory: a global memory created from the Memory screen, held by the daemon and
// found again by the screen's search.

import assert from "node:assert/strict";
import { expect, test } from "@playwright/test";
import { until } from "../../harness/process.ts";
import { aetherConn } from "../flows/desktop/state.ts";
import { memoryTexts } from "./daemon.ts";
import { connectionsReady, journeyHooks } from "./journey.ts";
import { choose, confirmIfAsked, finish, need, open, textField } from "./ui.ts";

const journey = journeyHooks({ archon: false });
const TEXT = "Journey memory: the release branch is cut on Thursdays.";
const SEARCH = "release branch";

test("memory journey: create a global memory, then find it by search", async () => {
  const { window, stack } = journey();
  await connectionsReady(journey(), false);
  await open(window, "Memory", `#/memory/${encodeURIComponent(await aetherConn(stack))}`);
  const create = window.getByRole("button", { name: "Create memory", exact: true });
  const sheet = window.getByRole("dialog", { name: "Create memory" });
  if (await create.isVisible()) await create.click();
  await (await textField(await need(window, "memory-create-text", () => sheet.getByLabel("Memory text", { exact: true })))).fill(TEXT);
  const scope = window.getByTestId("memory-create-scope").or(sheet.getByRole("tablist", { name: "Memory scope" })).first();
  await choose(window, scope, "Global");
  await (await need(window, "memory-create-submit", () => sheet.getByRole("button", { name: "Save memory", exact: true }))).click();
  await confirmIfAsked(window, "Save memory");
  const held = await until(() => memoryTexts(stack, SEARCH), (texts) => texts.includes(TEXT), "the memory on the daemon", 30_000);
  assert.equal(held.filter((text) => text === TEXT).length, 1, "one memory was created");
  await (await textField(await need(window, "memory-search", (p) => p.getByLabel("Search memories", { exact: true })))).fill(SEARCH);
  await expect(window.getByText(TEXT).first(), "the search finds the memory").toBeVisible();
  finish();
});
