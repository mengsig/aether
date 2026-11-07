// Flow 3, an observer device: paired as observer with both daemons, the page reads everything and
// is refused every operate command, by its own controls and, sent anyway on its socket, by the daemons.

import { AETHER_OPERATE } from "../../../apps/desktop/src/security/classes.ts";
import { createAgent, enqueueItem, pauseQueue } from "../../harness/agents.ts";
import { pairBoth } from "../../harness/pair.ts";
import { expect, test } from "../../harness/fixtures.ts";
import { openAgent, openItem } from "../noesis/noesis.ts";
import { snapshot } from "./desktop/state.ts";
import { ProtocolSchema, SocketTap, type Answer, type Frame } from "./wire.ts";

const AGENT = "Observed agent";
const ITEM = "Observed item";
const DEVICE = "Flow observer";
/** What a device may change on archond (archon-remote allow.rs): every one is refused to an observer. */
const ARCHON_CHANGES = ["enqueue", "cancel_item", "retry_item", "pause", "resume", "link_issue", "unlink_issue", "import_issue"] as const;

/** aetherd validates a frame before it checks the role, so a few commands need more than their schema. */
function aetherFrame(schema: ProtocolSchema, cmd: string, agent: string): Frame {
  const frame = schema.frame(cmd);
  if (cmd !== "create_schedule") return frame;
  return { ...frame, spec: { ...(frame.spec as object), target: agent } };
}

function refusal(answer: Answer): string {
  expect.soft(answer.ok, `refused, not answered: ${JSON.stringify(answer.ok)}`).toBeUndefined();
  return JSON.stringify(answer.err);
}

test("an observer device reads both daemons and is refused every operate command, in the UI and on the wire", async ({ page, stack }, info) => {
  test.setTimeout(150_000);
  await pauseQueue(stack);
  const agentId = await createAgent(stack, AGENT, "Be watched by an observer.", false);
  const itemId = await enqueueItem(stack, ITEM);
  const tap = await SocketTap.install(page);
  await pairBoth(page, stack, { aether: "observer", archon: "observer" }, DEVICE);

  const devices = await stack.aether.request({ cmd: "list_devices" });
  const mine = (devices.data as unknown as { name: string; role: string }[]).filter((device) => device.name === DEVICE);
  expect(mine.map((device) => device.role), "aetherd enrolled the page as an observer").toEqual(["observer"]);
  const remote = await stack.archon.request({ cmd: "remote_status" });
  const archonDevices = (remote.data.devices as { name: string; role?: string }[] | undefined) ?? [];
  expect(archonDevices.filter((device) => device.name === DEVICE).map((device) => device.role), "archond paired the page as an observer")
    .toEqual(["observer"]);

  // Reads work: the fleet and the queue show what the daemons hold.
  const conn = await openAgent(page, stack, AGENT);
  await page.goto(`${stack.aether.origin}/#/controls/${encodeURIComponent(conn)}/${agentId}`);
  await expect(page.getByRole("note").filter({ hasText: "Every control here is read-only on this device" })).toBeVisible();
  const actions = page.locator(".b3-action");
  expect(await actions.count(), "the controls screen offers its commands").toBeGreaterThan(0);
  for (const action of await actions.all()) {
    await expect(action, "every agent control is refused").toHaveAttribute("data-refused", "true");
    await expect(action.getByRole("button")).toBeDisabled();
  }
  await page.screenshot({ path: info.outputPath("observer-controls.png") });

  await page.goto(`${stack.aether.origin}/#/archon/queue`);
  await expect(page.getByRole("region", { name: "Queue" }).getByRole("link", { name: ITEM, exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "New item" }), "an observer cannot queue work").toBeDisabled();
  await expect(page.getByRole("button", { name: /new work$/ }), "an observer cannot pause or resume").toBeDisabled();
  await openItem(page, stack, ITEM);
  const itemButtons = page.getByRole("group", { name: "Item actions" }).getByRole("button");
  expect(await itemButtons.count(), "the item offers its actions").toBeGreaterThan(0);
  for (const button of await itemButtons.all()) await expect(button, "every item action is refused").toBeDisabled();
  await page.screenshot({ path: info.outputPath("observer-item.png") });

  // The wire: every operate command, sent on the page's own authenticated sockets, is refused.
  const before = await snapshot(stack);
  const aether = new ProtocolSchema("aether");
  for (const cmd of [...AETHER_OPERATE].sort()) {
    const why = refusal(await tap.send("aether", aetherFrame(aether, cmd, agentId)));
    expect.soft(why, `aetherd refuses ${cmd} to an observer`).toContain("forbidden");
    expect.soft(why, `aetherd refuses ${cmd} for its role`).toContain("needs the operator role");
  }
  const archon = new ProtocolSchema("archon");
  const item = { item: itemId };
  for (const cmd of ARCHON_CHANGES) {
    const sample = archon.frame(cmd);
    const frame = "item" in sample ? { ...sample, ...item } : sample;
    const why = refusal(await tap.send("archon", frame));
    expect.soft(why, `archond refuses ${cmd} to an observer`).toContain("this device only observes");
  }
  const reads = await tap.send("aether", aether.frame("list_agents", { query: {} }));
  expect(reads.ok, "the same socket still reads").toBeDefined();
  expect(await snapshot(stack), "neither daemon changed").toEqual(before);
});
