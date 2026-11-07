// Daemon-side reads the desktop flows assert on, over the host sockets, beside harness/agents.ts:
// item states and history, pause, devices and roles. Each checks the reply it reads.

import assert from "node:assert/strict";
import type { Page } from "@playwright/test";
import { listAgents, queuePaused, type AgentRow } from "../../../harness/agents.ts";
import type { Stack } from "../../../harness/stack.ts";

export interface ItemRow { readonly id: string; readonly title: string; readonly state: string }

/** Who archond says made an item transition (`TransitionActor`). */
export type TransitionActor = "daemon" | "cli" | "web" | "mcp" | "github";
export interface Transition { readonly seq: number; readonly from: string | null; readonly to: string; readonly by: TransitionActor }

/** The id core gives aetherd's connection: `aether:<instance_id>`, which routes name. */
export async function aetherConn(stack: Stack): Promise<string> {
  const welcome = await stack.aether.welcome();
  assert.equal(welcome.reply, "welcome");
  assert.match(String(welcome.data.instance_id), /^\S+$/, "the welcome names the daemon instance");
  return `aether:${String(welcome.data.instance_id)}`;
}

export async function items(stack: Stack): Promise<ItemRow[]> {
  const { reply, data } = await stack.archon.request({ cmd: "list_item_summaries" });
  assert.equal(reply, "item_summaries");
  assert.ok(Array.isArray(data), "list_item_summaries answers with an array");
  return (data as Array<{ id: string; title: string; state: { state: string } }>)
    .map(({ id, title, state }) => ({ id, title, state: state.state }))
    .sort((a, b) => a.id.localeCompare(b.id));
}

export async function itemState(stack: Stack, id: string): Promise<string> {
  const found = (await items(stack)).find((item) => item.id === id);
  assert.ok(found !== undefined, `archond holds item ${id}`);
  return found.state;
}

export async function itemHistory(stack: Stack, id: string): Promise<Transition[]> {
  const { reply, data } = await stack.archon.request({ cmd: "item_history", item: id });
  assert.equal(reply, "item_history", `item_history answers: ${reply}`);
  assert.equal(data.item, id, "the history is the item's own");
  const transitions = data.transitions as Transition[];
  assert.ok(Array.isArray(transitions) && transitions.length > 0, "an item's history starts with its creation");
  for (let i = 1; i < transitions.length; i += 1) {
    assert.ok((transitions[i] as Transition).seq > (transitions[i - 1] as Transition).seq, "transitions are oldest first");
  }
  return transitions;
}

/** The transition that last moved `id` into `to`. */
export async function lastTransitionTo(stack: Stack, id: string, to: string): Promise<Transition> {
  const found = (await itemHistory(stack, id)).filter((transition) => transition.to === to).at(-1);
  assert.ok(found !== undefined, `item ${id} moved to ${to}`);
  return found;
}

/** Sets the global pause from the host socket; a no-op when it already holds. */
export async function setPaused(stack: Stack, paused: boolean): Promise<void> {
  if ((await queuePaused(stack)) === paused) return;
  await stack.archon.request({ cmd: paused ? "pause" : "resume", scope: "global", hard: false });
  assert.equal(await queuePaused(stack), paused, `archond reports the queue ${paused ? "paused" : "running"}`);
}

/** Everything an observer could have changed: agents, items with their states, and the pause. */
export interface Snapshot { readonly agents: AgentRow[]; readonly items: ItemRow[]; readonly paused: boolean }

export async function snapshot(stack: Stack): Promise<Snapshot> {
  const agents = (await listAgents(stack)).sort((a, b) => a.id.localeCompare(b.id));
  return { agents, items: await items(stack), paused: await queuePaused(stack) };
}

export interface DeviceRole { readonly id: string; readonly name: string; readonly role: string }

/** aetherd's paired devices with their roles (`list_devices`, the host's own socket). */
export async function aetherDevices(stack: Stack): Promise<DeviceRole[]> {
  const { reply, data } = await stack.aether.request({ cmd: "list_devices" });
  assert.equal(reply, "devices", `list_devices answers devices: ${reply}`);
  const devices = (Array.isArray(data) ? data : (data as { devices?: unknown }).devices) as DeviceRole[] | undefined;
  assert.ok(Array.isArray(devices), `list_devices lists devices: ${JSON.stringify(data).slice(0, 400)}`);
  return devices.map(({ id, name, role }) => ({ id: String(id), name, role }));
}

/** archond's paired devices with their roles (`remote_status`). */
export async function archonDevices(stack: Stack): Promise<DeviceRole[]> {
  const { reply, data } = await stack.archon.request({ cmd: "remote_status" });
  assert.equal(reply, "remote", `remote_status answers remote: ${reply}`);
  const devices = data.devices as DeviceRole[] | undefined;
  assert.ok(Array.isArray(devices), "remote_status lists the paired devices");
  return devices.map(({ id, name, role }) => ({ id, name, role }));
}

/** Moves a window's single-page app to `hash` without a reload. */
export async function route(window: Page, hash: string): Promise<void> {
  assert.ok(hash.startsWith("#/"), `an app route is a hash route: ${hash}`);
  await window.evaluate((next) => { location.hash = next; }, hash);
}
