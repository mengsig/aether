// Journey 5, memory: a global memory created from the Memory screen, held by the daemon and
// found again by the screen's search.

import assert from "node:assert/strict";
import { expect, test } from "@playwright/test";
import { until } from "../../harness/process.ts";
import { aetherConn } from "../flows/desktop/state.ts";
import { memoryTexts } from "./daemon.ts";
import { connectionsReady, journeyHooks } from "./journey.ts";
import { shootWindow } from "./capture.ts";
import { choose, need, open, textField } from "./ui.ts";

const journey = journeyHooks({ archon: false });
const TEXT = "Journey memory: the release branch is cut on Thursdays.";
const SEARCH = "release branch";

test("memory journey: create a global memory, then find it by search", async () => {
  const { window, stack } = journey();
  await connectionsReady(journey(), false);
  await open(window, "Memory", `#/memory/${encodeURIComponent(await aetherConn(stack))}`);
  const create = window.getByRole("button", { name: "Create memory", exact: true });
  const sheet = window.getByRole("dialog", { name: "Create memory" });
  await expect(create, "the Memory screen offers Create memory").toBeVisible();
  await create.click();
  await expect(sheet, "the create sheet opens").toBeVisible();
  await (await textField(await need(sheet, "memory-create-text"))).fill(TEXT);
  await choose(window, sheet.getByRole("radiogroup", { name: "Memory scope" }), "Global");
  await expect(sheet.getByTestId("memory-create-repository"), "a global memory names no repository").toHaveCount(0);
  await shootWindow(window, "memory", "create-sheet");
  const submit = await need(sheet, "memory-create-submit");
  await expect(submit, "the memory is ready to save").toBeEnabled();
  await submit.click();
  await expect(sheet, "saving closes the sheet").toHaveCount(0);
  const held = await until(() => memoryTexts(stack, SEARCH), (texts) => texts.includes(TEXT), "the memory on the daemon", 30_000);
  assert.equal(held.filter((text) => text === TEXT).length, 1, "one memory was created");
  await (await textField(await need(window, "memory-search"))).fill(SEARCH);
  await expect(window.getByText(TEXT).first(), "the search finds the memory").toBeVisible();
  await shootWindow(window, "memory", "search-hit");
});
