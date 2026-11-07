// The Learn screens against a real daemon with Noesis on: the off-state of Learn from my work turns
// it on in place, the settings pick models from what the daemon offers, and the graph reads as a list.
// No lesson runs here: the faux engine streams prose, never Noesis's tool calls.

import { createAgent } from "../../harness/agents.ts";
import { pairBoth } from "../../harness/pair.ts";
import { expect, test } from "../../harness/fixtures.ts";
import { configureNoesis, noesisChanges, noesisStatus, openAgent, recordCommands } from "./noesis.ts";

const AGENT = "Noesis learn agent";

test.beforeAll(async ({ stack }) => {
  await createAgent(stack, AGENT, "Stay idle while the Learn screens are checked.", false);
  const status = await configureNoesis(stack, { enabled: true, engineering_context: false });
  expect(status.effective.engineering_context, "Learn from my work starts off").toBe(false);
});

test.afterAll(async ({ stack }) => {
  const status = await configureNoesis(stack, { enabled: false, engineering_context: false });
  expect(status.enabled, "Noesis is back off for the specs after this file").toBe(false);
});

test("a Learn from this link with Learn from my work off turns it on in place and asks no model", async ({ page, stack }) => {
  const sent = recordCommands(page);
  await pairBoth(page, stack, { aether: "operator" });
  const conn = await openAgent(page, stack, AGENT);
  const agent = /agt_[0-9A-Z]{26}/.exec(page.url())?.[0];
  expect(agent, "the agent route names the agent").toBeDefined();
  const requests = stack.engine.requests.length;
  const from = sent.commands.length;

  await page.goto(`${stack.aether.origin}/#/learn?understand=agent&conn=${encodeURIComponent(conn)}&id=${agent ?? ""}`);
  const sheet = page.getByRole("dialog", { name: "Learn from this" });
  await expect(sheet.getByTestId("understand-off")).toContainText("only what you pick is shown to the teaching model");
  await sheet.getByRole("button", { name: "Turn on Learn from my work" }).click();
  await expect(sheet.getByTestId("understand-off"), "the preview replaces the off note").toHaveCount(0);

  await expect.poll(async () => (await noesisStatus(stack)).effective.engineering_context, { message: "the daemon turned it on" }).toBe(true);
  expect(noesisChanges(sent.commands.slice(from)), "the switch is one configure").toEqual(["noesis_configure"]);
  expect(stack.engine.requests.length, "turning it on asked no model").toBe(requests);
  await configureNoesis(stack, { engineering_context: false });
});

test("the settings pick each model from the daemon's offers and say what today was billed to", async ({ page, stack }) => {
  await pairBoth(page, stack);
  await page.goto(`${stack.aether.origin}/#/learn?panel=settings`);
  const sheet = page.getByRole("dialog", { name: "Noesis settings" });
  for (const select of ["#learn-model-strong", "#learn-model-cheap"]) {
    const chooser = sheet.locator(select);
    await expect(chooser, `${select} is a select, not free text`).toHaveJSProperty("tagName", "SELECT");
    await expect(chooser.locator("option").first()).toHaveText("Automatic — the first ready subscription model");
  }
  await expect(sheet.locator("#learn-model-strong-note"), "the note says what teaching runs on, or why it cannot").toContainText(/Teaching (runs on|cannot run now)/);
  await expect(sheet.getByLabel("Billing today")).toContainText(/No model use recorded yet|subscription|API key/);
  await expect(sheet.getByRole("switch", { name: /Learn from my work/ })).not.toBeChecked();
});

test("the knowledge graph switches between the drawing and a list of the same propositions", async ({ page, stack }) => {
  await pairBoth(page, stack);
  await page.goto(`${stack.aether.origin}/#/learn/graph`);
  const shown = page.getByRole("group", { name: "Show as" });
  await expect(shown.getByRole("button", { name: "Graph" })).toHaveAttribute("aria-pressed", "true");
  await shown.getByRole("button", { name: "List" }).click();
  await expect(shown.getByRole("button", { name: "List" })).toHaveAttribute("aria-pressed", "true");
});
