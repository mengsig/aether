import assert from "node:assert/strict";
import { test } from "node:test";
import { ConnectionTable, type LocalDaemonRecord } from "../../src/bridge/connections.ts";
import type { DaemonLink } from "../../src/bridge/daemon-link.ts";
import { sendToOwner } from "../../src/main/owner-events.ts";

const daemon: LocalDaemonRecord = { id: "aether", kind: "aether", label: "Local", socket: "/private/aether.sock" };

function setup() {
  const events: Array<[number, string]> = [];
  const links: Array<{ isOpen: boolean; onClose: () => void; close(): void }> = [];
  const table = new ConnectionTable({
    openLink: async () => {
      const link = { isOpen: true, onClose: () => {}, close() { this.isOpen = false; this.onClose(); } };
      links.push(link);
      return link as unknown as DaemonLink;
    },
    push: () => {},
    closed: (owner, conn) => events.push([owner, conn]),
  });
  return { table, links, events };
}

test("an unexpected close notifies only its owner once", async () => {
  const { table, links, events } = setup();
  const first = await table.connect(1, daemon);
  const second = await table.connect(2, daemon);
  const link = links[0];
  assert.ok(link);
  link.close();
  link.onClose();
  assert.deepEqual(events, [[1, first]]);
  assert.equal(table.kindOf(first), undefined);
  assert.equal(table.kindOf(second), "aether");
});

test("explicit and window closes never notify", async () => {
  const { table, links, events } = setup();
  const first = await table.connect(1, daemon);
  const second = await table.connect(2, daemon);
  table.close(1, first);
  table.closeOwner(2);
  assert.deepEqual(events, []);
  assert.equal(links.every((link) => !link.isOpen), true);
  assert.equal(table.kindOf(second), undefined);
});

test("a link already closed during setup is rejected", async () => {
  const { table, links } = setup();
  const opening = table.connect(1, daemon);
  const link = links[0];
  assert.ok(link);
  link.close();
  await assert.rejects(opening, /closed during connection setup/);
});

test("delivery targets the owner and skips destroyed or missing windows", () => {
  const sent: Array<[number, string, unknown[]]> = [];
  const find = (id: number) => id === 3 ? undefined : {
    isDestroyed: () => id === 2,
    send: (channel: string, ...args: unknown[]) => sent.push([id, channel, args]),
  };
  assert.equal(sendToOwner(find, 1, "desktop:closed", "c1"), true);
  assert.equal(sendToOwner(find, 2, "desktop:closed", "c2"), false);
  assert.equal(sendToOwner(find, 3, "desktop:closed", "c3"), false);
  assert.deepEqual(sent, [[1, "desktop:closed", ["c1"]]]);
});
