// Pairing: a fresh browser pairs with aetherd through its printed URL and with archond through
// the /archon mount, and each daemon records exactly the device the page claimed.

import { connection, pairAether, pairArchon } from "../../harness/pair.ts";
import { expect, test } from "../../harness/fixtures.ts";

test("a fresh browser pairs with both daemons through Aether's listener", async ({ page, stack }, info) => {
  const device = await pairAether(page, stack, "Smoke pairing");
  const devices = await stack.aether.request({ cmd: "list_devices" });
  expect(JSON.stringify(devices.data)).toContain(device);

  await pairArchon(page, stack, "Smoke pairing");
  const remote = await stack.archon.request({ cmd: "remote_status" });
  expect(JSON.stringify(remote.data)).toContain("Smoke pairing");

  await page.goto(`${stack.aether.origin}/#/fleet`);
  await expect(connection(page, "aether")).toHaveAttribute("data-state", "ready");
  await expect(connection(page, "archon")).toHaveAttribute("data-state", "ready");
  await page.screenshot({ path: info.outputPath("paired.png") });
});
