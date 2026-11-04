// A deterministic synthetic fleet for the tests, the perf page and the screenshots: Archon runs
// (`archon-run:*` roots with `archon-stage:*` children) mixed with plain runs, in every state.

import type { Store } from "@aether-ui/core";
import type { aether, ConnectionInfo } from "@aether-ui/protocol";
import { invariant } from "../invariant.ts";

const MODELS: readonly aether.ModelRef[] = [
  { provider: "claude-code", model: "opus" },
  { provider: "claude-code", model: "sonnet" },
  { provider: "codex", model: "gpt-6-astra" },
];
const ARCHON_STAGES = ["plan", "build", "review", "ship"];
const PLAIN_ROLES = ["worker", "researcher", "reviewer"];

const pad = (n: number): string => String(n).padStart(5, "0");

function statusAt(i: number, parent: string | undefined): aether.AgentStatus {
  switch (i % 8) {
    case 0: return { state: "running" };
    case 1: return { state: "ready" };
    case 2: return { state: "completed" };
    case 3: return parent ? { state: "waiting", condition: { on: "message_from", agent: parent } } : { state: "running" };
    case 4: return { state: "blocked", reason: { block: "inspection", detail: "waiting for an operator check" } };
    case 5: return { state: "failed", error: "the engine exited" };
    case 6: return { state: "cancelled" };
    default: return { state: "created" };
  }
}

/** The size of run `r`: between 3 and 12 agents, so runs differ and none starts collapsed. */
const runSize = (r: number): number => 3 + ((r * 7) % 10);

function roleOf(archon: boolean, member: number): string {
  if (archon) return member === 0 ? "archon-run:adaptive" : `archon-stage:adaptive/${ARCHON_STAGES[(member - 1) % ARCHON_STAGES.length]}`;
  return member === 0 ? "operator" : PLAIN_ROLES[(member - 1) % PLAIN_ROLES.length] ?? "worker";
}

/** `count` agents keyed by id; every third run is an Archon run, and children delegate up to three deep. */
export function fixtureAgents(count: number): Record<string, aether.Agent> {
  invariant(Number.isInteger(count) && count >= 0 && count < 100_000, `fixture size ${count} is a whole number below 100,000`);
  const agents: Record<string, aether.Agent> = {};
  let run = 0;
  let member = 0;
  let root = "";
  for (let i = 0; i < count; i += 1) {
    if (member === runSize(run)) { run += 1; member = 0; }
    const id = `agt_${pad(i)}`;
    if (member === 0) root = id;
    const archon = run % 3 === 0;
    // Members 1-3 report to the root; later members to one of them.
    const parent = member === 0 ? undefined : member <= 3 ? root : `agt_${pad(i - member + 1 + ((member - 1) % 3))}`;
    const role = roleOf(archon, member);
    const name = member === 0 ? (archon ? `qi-${pad(run)}` : `run-${pad(run)}`) : `${role.slice(role.lastIndexOf("/") + 1)}-${member}`;
    agents[id] = {
      id, name, role, run: `run_${pad(run)}`, model: MODELS[i % MODELS.length] ?? { provider: "fake", model: "fake" },
      status: statusAt(i, parent), config: {}, scope: {}, created_at: "2026-11-04T08:00:00Z", updated_at: "2026-11-04T08:00:00Z",
      ...(parent ? { parent } : {}), ...(i % 23 === 5 ? { paused: true } : {}),
    };
    member += 1;
  }
  invariant(Object.keys(agents).length === count, "every fixture agent has its own id");
  return agents;
}

export const FIXTURE_CONN = "aether:fixture";

export function fixtureConnection(id = FIXTURE_CONN, label = "studio.local"): ConnectionInfo {
  return { id, daemon: "aether", state: "ready", protocol: 26, features: ["agents_paging"], standing: "operator", label };
}

/** Puts a connection and its agents into a store, as core's snapshot read would. */
export function seedFleet(store: Store, info: ConnectionInfo, agents: Record<string, aether.Agent>): void {
  invariant(info.daemon === "aether", "the fleet fixture seeds Aether connections");
  store.update((state) => ({ ...state, connections: { ...state.connections, [info.id]: info } }));
  const roots = Object.values(agents).filter((agent) => !agent.parent).map((agent) => agent.id);
  store.updateAether(info.id, (state) => ({ ...state, agents, roots }));
  invariant(store.get().aether[info.id]?.agents === agents, "the store holds the seeded agents");
}
