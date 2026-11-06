// Daemon-side reads and setup for the journeys, over the host sockets, so each journey asserts
// what the daemons hold and not only what the window draws. Every read checks its reply.

import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { promisify } from "node:util";
import type { Stack } from "../../harness/stack.ts";

const run = promisify(execFile);

export interface AgentView {
  readonly id: string;
  readonly name: string;
  readonly paused: boolean;
  readonly state: string;
  readonly model: { readonly provider: string; readonly model: string };
  readonly usage: { readonly cost_usd?: number | null; readonly estimated_cost_usd?: number | null; readonly input_tokens?: number; readonly output_tokens?: number } | null;
}

async function list(stack: Stack, cmd: Record<string, unknown> & { cmd: string }, reply: string): Promise<unknown[]> {
  const answer = await stack.aether.request(cmd);
  assert.equal(answer.reply, reply, `${cmd.cmd} answers ${reply}: ${answer.reply}`);
  const rows = Array.isArray(answer.data) ? answer.data : (answer.data as Record<string, unknown>)[reply];
  assert.ok(Array.isArray(rows), `${cmd.cmd} lists rows: ${JSON.stringify(answer.data).slice(0, 400)}`);
  return rows;
}

export async function agent(stack: Stack, id: string): Promise<AgentView> {
  assert.match(id, /^agt_/, `an agent id: ${id}`);
  const { reply, data } = await stack.aether.request({ cmd: "get_agent", id });
  assert.equal(reply, "agent", `get_agent answers agent: ${reply}`);
  const raw = data as { id: string; name: string; paused?: boolean; status: { state: string }; model: AgentView["model"]; stats?: { usage?: AgentView["usage"] } };
  assert.equal(raw.id, id, "the daemon answered for the agent asked about");
  return { id, name: raw.name, paused: raw.paused === true, state: raw.status.state, model: raw.model, usage: raw.stats?.usage ?? null };
}

export async function agentIds(stack: Stack): Promise<string[]> {
  const rows = await list(stack, { cmd: "list_agents", query: {} }, "agents") as Array<{ id: string }>;
  return rows.map((row) => row.id);
}

export async function pendingApprovals(stack: Stack, agentId: string): Promise<Array<{ id: string; tool: string }>> {
  const rows = await list(stack, { cmd: "list_approvals" }, "approvals") as Array<{ id: string; tool: string; agent_id: string; status: unknown }>;
  return rows.filter((row) => row.agent_id === agentId && JSON.stringify(row.status).includes("pending")).map(({ id, tool }) => ({ id, tool }));
}

export async function approvalRuleTools(stack: Stack): Promise<string[]> {
  const rows = await list(stack, { cmd: "list_approval_rules" }, "approval_rules") as Array<{ match?: { tool?: string | null } }>;
  return rows.map((rule) => String(rule.match?.tool ?? ""));
}

/** Names of the schedules that will still fire: active or paused, not cancelled or ended. */
export async function scheduleNames(stack: Stack): Promise<string[]> {
  const rows = await list(stack, { cmd: "list_schedules" }, "schedules") as Array<{ name: string; status: { state: string } }>;
  assert.ok(rows.every((row) => typeof row.name === "string" && typeof row.status?.state === "string"), "each schedule has a name and a status");
  return rows.filter((row) => row.status.state === "active" || row.status.state === "paused").map((row) => row.name);
}

export async function memoryTexts(stack: Stack, text: string): Promise<string[]> {
  assert.ok(text.trim() !== "", "a memory search has text");
  // Not `proposed`: that lists only proposals, and a memory the host creates is active at once.
  const rows = await list(stack, { cmd: "memories", query: { text } }, "memories") as Array<{ text?: string; memory?: { text?: string } }>;
  return rows.map((row) => String(row.text ?? row.memory?.text ?? ""));
}

export async function taskObjectives(stack: Stack, owner: string): Promise<string[]> {
  const rows = await list(stack, { cmd: "list_tasks", query: { owner } }, "tasks") as Array<{ objective: string }>;
  return rows.map((task) => task.objective);
}

export interface Spend {
  readonly agents: number;
  readonly usd: number;
  /** Agents that ran but reported neither an actual nor an estimated cost. */
  readonly unreported: number;
}

/** What every agent on the stack spent: actual cost where the engine reported it, else its estimate. */
export async function stackSpend(stack: Stack): Promise<Spend> {
  const views = await Promise.all((await agentIds(stack)).map((id) => agent(stack, id)));
  let usd = 0;
  let unreported = 0;
  for (const view of views) {
    const cost = view.usage?.cost_usd ?? view.usage?.estimated_cost_usd ?? null;
    if (cost === null) unreported += view.usage === null ? 0 : 1;
    else usd += cost;
  }
  assert.ok(Number.isFinite(usd) && usd >= 0, `spend is a non-negative amount: ${usd}`);
  return { agents: views.length, usd, unreported };
}

/** Cancels each agent that is still live; the journeys' cheap cleanup before the stack stops. */
export async function cancelLive(stack: Stack, ids: readonly string[]): Promise<void> {
  for (const id of ids) {
    const view = await agent(stack, id);
    if (["completed", "failed", "cancelled"].includes(view.state)) continue;
    await stack.aether.request({ cmd: "cancel", agent: id, reason: "journey cleanup" });
  }
}

/** A scratch git repository with one commit and no remote, inside the stack root. */
export async function scratchRepo(stack: Stack, name: string): Promise<string> {
  assert.match(name, /^[a-z0-9-]+$/, `a scratch repository name is plain: ${name}`);
  const repo = path.join(stack.layout.root, "repos", name);
  await mkdir(repo, { recursive: true, mode: 0o700 });
  const git = (...args: string[]) => run("git", ["-C", repo, ...args], { env: stack.layout.env, timeout: 15_000 });
  await git("init", "-q", "-b", "main");
  await writeFile(path.join(repo, "README.md"), `# ${name}\n\nA journey's scratch repository.\n`);
  await git("add", "README.md");
  await git("-c", "user.name=Journey", "-c", "user.email=journey@example.invalid", "commit", "-q", "-m", "init");
  const { stdout } = await git("remote");
  assert.equal(stdout.trim(), "", "the scratch repository has no remote");
  assert.ok(repo.startsWith(`${stack.layout.root}/`), "the scratch repository is inside the stack root");
  return repo;
}

export interface NewAgent {
  readonly name: string;
  /** Its first task; none leaves the agent free to take one (an agent carries one task at a time). */
  readonly objective?: string;
  readonly model: { readonly provider: string; readonly model: string };
  readonly start?: boolean;
  /** Side-effect classes that ask first; every class makes any tool call wait for approval. */
  readonly requireFor?: readonly string[];
  readonly limits?: { readonly cost_budget: number; readonly max_turns: number };
}

export async function createAgentOn(stack: Stack, spec: NewAgent): Promise<string> {
  assert.ok(spec.name.trim() !== "", "an agent has a name");
  assert.ok(spec.objective === undefined || spec.objective.trim() !== "", "an objective, when given, is not blank");
  const { reply, data } = await stack.aether.request({
    cmd: "create_agent", role: "journey-worker", name: spec.name, ...(spec.objective === undefined ? {} : { objective: spec.objective }),
    model: spec.model, start: spec.start ?? true,
    scope: { workspace_root: stack.layout.aether.work },
    ...(spec.requireFor === undefined ? {} : { approval: { require_for: spec.requireFor } }),
    ...(spec.limits === undefined ? {} : { config: { ...spec.limits } }),
  });
  assert.equal(reply, "agent", `create_agent answers with the agent: ${reply}`);
  assert.match(String(data.id), /^agt_/);
  return String(data.id);
}

/** The agent's transcript as one string, for checks that a tool result or reply is in it. */
export async function transcriptText(stack: Stack, id: string): Promise<string> {
  const { reply, data } = await stack.aether.request({ cmd: "get_transcript", agent: id, after: 0 });
  assert.equal(reply, "transcript", `get_transcript answers transcript: ${reply}`);
  const text = JSON.stringify(data);
  assert.ok(text.length > 2, "the transcript is not empty");
  return text;
}

/** Caps an agent the window started: the real variant's cost budget and turn limit. */
export async function capAgent(stack: Stack, id: string, limits: { readonly cost_budget: number; readonly max_turns: number }): Promise<void> {
  assert.ok(limits.cost_budget > 0 && limits.max_turns > 0, "the caps are positive");
  const { reply } = await stack.aether.request({ cmd: "update_agent", id, patch: { ...limits } });
  assert.equal(reply, "agent", `update_agent answers with the agent: ${reply}`);
}
