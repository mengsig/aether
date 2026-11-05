// The fleet: agents aetherd runs on the faux engine (Archon's orchestrator and one created
// here) appear as the daemon lists them, live, with the model they run on.

import { createAgent, listAgents } from "../../harness/agents.ts";
import { pairBoth } from "../../harness/pair.ts";
import { expect, test } from "../../harness/fixtures.ts";

test("the fleet lists the daemon's agents live, on the faux engine", async ({ page, stack }, info) => {
  await pairBoth(page, stack);
  const fleet = page.getByRole("region", { name: "Fleet" });
  await expect(fleet.getByRole("link", { name: "Archon", exact: true })).toBeVisible();

  const requests = stack.engine.requests.length;
  await createAgent(stack, "Smoke worker", "Say hello from the fleet smoke test.");
  const row = fleet.locator(".fleet-row", { has: page.getByRole("link", { name: "Smoke worker" }) });
  await expect(row).toBeVisible();
  await expect(row.locator(".fleet-model")).toHaveText("mock/mock-1");
  await expect.poll(() => stack.engine.requests.slice(requests).some((request) => request.lastUserText.includes("fleet smoke test"))).toBe(true);

  const agents = await listAgents(stack);
  await expect(fleet.locator(".fleet-count")).toHaveText(`${agents.length} of ${agents.length} agents`);
  await page.screenshot({ path: info.outputPath("fleet.png") });
});
