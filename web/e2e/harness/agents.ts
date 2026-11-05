// Daemon-side state the specs set and check over the host sockets, so a spec asserts what the
// daemon holds and not only what the page draws.

import assert from "node:assert/strict";
import type { Stack } from "./stack.ts";

export interface AgentRow { readonly id: string; readonly name: string }

const FAUX_MODEL_REF = { provider: "mock", model: "mock-1" } as const;

/** Creates an agent on the faux engine, started unless `start` is false (it then stays live); returns its id. */
export async function createAgent(stack: Stack, name: string, objective: string, start = true): Promise<string> {
  assert.ok(name.trim() !== "" && objective.trim() !== "", "an agent has a name and an objective");
  const { reply, data } = await stack.aether.request({ cmd: "create_agent", role: "e2e-worker", name, objective, model: FAUX_MODEL_REF, start });
  assert.equal(reply, "agent", `create_agent answers with the agent: ${reply}`);
  assert.equal(data.name, name);
  assert.match(String(data.id), /^agt_/);
  return String(data.id);
}

export async function renameAgent(stack: Stack, id: string, name: string): Promise<void> {
  const { reply, data } = await stack.aether.request({ cmd: "update_agent", id, patch: { name } });
  assert.equal(reply, "agent", `update_agent answers with the agent: ${reply}`);
  assert.equal(data.name, name, "the daemon holds the new name");
}

export async function listAgents(stack: Stack): Promise<AgentRow[]> {
  const { reply, data } = await stack.aether.request({ cmd: "list_agents", query: {} });
  assert.equal(reply, "agents");
  assert.ok(Array.isArray(data), "list_agents answers with an array");
  return (data as { id: string; name: string }[]).map(({ id, name }) => ({ id, name }));
}

export async function queuePaused(stack: Stack): Promise<boolean> {
  const { reply, data } = await stack.archon.request({ cmd: "status" });
  assert.equal(reply, "status");
  const pause = data.pause as { global?: unknown } | undefined;
  assert.equal(typeof pause?.global, "boolean", `status reports the global pause: ${JSON.stringify(pause)}`);
  return pause?.global === true;
}

/** Queues an `ask` item, which needs no repository; with the queue paused it stays queued. */
export async function enqueueItem(stack: Stack, title: string): Promise<string> {
  assert.ok(title.trim() !== "", "an item has a title");
  const { reply, data } = await stack.archon.request({
    cmd: "enqueue", repo: null, pipeline: "ask", title, brief: `E2E item: ${title}`, priority: "normal", budget_usd: null,
    idempotency_key: `e2e-${title}`,
  });
  assert.equal(reply, "item", `enqueue answers with the item: ${reply}`);
  assert.match(String(data.id), /^qi_/);
  return String(data.id);
}

export async function retitleItem(stack: Stack, item: string, title: string): Promise<void> {
  const { reply, data } = await stack.archon.request({ cmd: "update_item", item, patch: { title } });
  assert.equal(reply, "item", `update_item answers with the item: ${reply}`);
  assert.equal(data.title, title, "the daemon holds the new title");
}

export async function itemTitles(stack: Stack): Promise<string[]> {
  const { reply, data } = await stack.archon.request({ cmd: "list_item_summaries" });
  assert.equal(reply, "item_summaries");
  assert.ok(Array.isArray(data), "list_item_summaries answers with an array");
  return (data as { title: string }[]).map((item) => item.title).sort();
}

export async function pauseQueue(stack: Stack): Promise<void> {
  await stack.archon.request({ cmd: "pause", scope: "global", hard: false });
  assert.equal(await queuePaused(stack), true, "archond reports the queue paused");
}
