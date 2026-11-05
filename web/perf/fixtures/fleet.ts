// The fleet fixture: `count` agents in seeded runs of 1 to 24, a third of them Archon runs, in every
// state and model, keyed by id as core's store holds them after `list_agents_page`.

import type { aether } from "@aether-ui/protocol";
import { pad, seeded, stamp } from "./random.ts";

export const FLEET_SIZE = 5_000;

const MODELS: readonly aether.ModelRef[] = [
  { provider: "claude-code", model: "opus" }, { provider: "claude-code", model: "sonnet" },
  { provider: "codex", model: "gpt-6-astra" }, { provider: "codex", model: "gpt-6-terra" },
];
const ROLES = ["worker", "researcher", "reviewer", "archon-stage:adaptive/build", "archon-stage:adaptive/review", "archon-stage:adaptive/plan"];
const STATES: readonly aether.AgentStatus[] = [
  { state: "running" }, { state: "running" }, { state: "ready" }, { state: "completed" }, { state: "completed" }, { state: "created" },
  { state: "blocked", reason: { block: "inspection", detail: "waiting for an operator check" } },
  { state: "failed", error: "the engine exited" }, { state: "cancelled" },
];

export const agentId = (i: number): string => `agt_${pad(i)}`;

export function fleetAgents(seed: number, count = FLEET_SIZE): Record<string, aether.Agent> {
  if (!Number.isInteger(count) || count < 0) throw new Error(`fleet size ${count} is not a whole number`);
  const random = seeded(seed);
  const agents: Record<string, aether.Agent> = {};
  let run = 0;
  let left = 0;
  let root = "";
  for (let i = 0; i < count; i += 1) {
    const id = agentId(i);
    if (left === 0) { run += 1; left = 1 + random.below(24); root = id; }
    left -= 1;
    const isRoot = root === id;
    const archon = run % 3 === 0;
    const role = isRoot ? (archon ? "archon-run:adaptive" : "operator") : random.pick(ROLES);
    agents[id] = {
      id, name: isRoot ? `run-${pad(run)}` : `${role.slice(role.lastIndexOf("/") + 1)}-${pad(i)}`, role, run: `run_${pad(run)}`,
      model: random.pick(MODELS), status: random.pick(STATES), config: {}, scope: {},
      created_at: stamp(i), updated_at: stamp(i), ...(isRoot ? {} : { parent: root }), ...(random.below(40) === 0 ? { paused: true } : {}),
    };
  }
  if (Object.keys(agents).length !== count) throw new Error("every fleet agent has its own id");
  return agents;
}

/** The roots core keeps beside the agents: every agent without a parent. */
export function fleetRoots(agents: Readonly<Record<string, aether.Agent>>): string[] {
  return Object.values(agents).filter((agent) => !agent.parent).map((agent) => agent.id);
}
