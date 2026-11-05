// The Learning screens with Noesis off: each shows the off state and opening them triggers
// nothing, so the page sends no Noesis command but `noesis_status`, asks no model, starts no job.

import { createAgent } from "../../harness/agents.ts";
import { pairBoth } from "../../harness/pair.ts";
import { expect, test } from "../../harness/fixtures.ts";
import { noesisCommands, noesisFiles, noesisStatus, openAgent, recordCommands } from "./noesis.ts";

const SCREENS = ["#/learn", "#/learn/graph", "#/learn/me", "#/learn/sessions/ns_e2e"];

test("every Learning screen shows the off state and sends nothing but noesis_status", async ({ page, stack }, info) => {
  const sent = recordCommands(page);
  await pairBoth(page, stack);
  expect(noesisCommands(sent.commands), "the page read noesis_status when it connected").toContain("noesis_status");
  expect((await noesisStatus(stack)).enabled, "Noesis is off").toBe(false);
  const name = "Noesis off learner";
  const id = await createAgent(stack, name, "Stay idle while Noesis is off.", false);
  const conn = await openAgent(page, stack, name);
  const requests = stack.engine.requests.length;
  const from = sent.commands.length;

  for (const screen of SCREENS) {
    await page.goto(`${stack.aether.origin}/${screen}`);
    await expect(page.getByTestId("noesis-off"), `${screen} shows the off state`).toContainText("Noesis is off");
  }
  await page.goto(`${stack.aether.origin}/#/learn?panel=settings`);
  const toggle = page.getByRole("dialog", { name: "Noesis settings" }).getByRole("switch", { name: /Turn on Noesis/ });
  await expect(toggle, "an observer cannot turn Noesis on from the page").toBeDisabled();
  await expect(toggle).not.toBeChecked();

  const agent = new URLSearchParams({ understand: "agent", conn, id });
  await page.goto(`${stack.aether.origin}/#/learn?${agent.toString()}`);
  await expect(page.getByTestId("understand-off"), "a Learn from this link shows Noesis off").toBeVisible();
  await page.screenshot({ path: info.outputPath("learn-off.png") });

  const during = noesisCommands(sent.commands.slice(from));
  expect(during.filter((cmd) => cmd !== "noesis_status"), "opening the screens sent no Noesis command but noesis_status").toEqual([]);
  expect(sent.unparsed, "every frame the page sent was a command").toEqual([]);
  expect(stack.engine.requests.length, "opening the screens asked no model").toBe(requests);
  const status = await noesisStatus(stack);
  expect(status.enabled, "Noesis is still off").toBe(false);
  expect(status.jobs_running, "no Noesis job was started").toBe(0);
  expect(await noesisFiles(stack), "noesis.db was not created").toEqual([]);
});
