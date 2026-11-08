// A small seeded world for the overview's tests and screenshots: one Aether and one archond
// connection with agents in every state, a day of events, queue items, Noesis and the watcher.

import type { ArchonItem, Store } from "@aether-ui/core";
import type { aether, archon, ConnectionInfo } from "@aether-ui/protocol";
import { invariant } from "../invariant.ts";

export const NOW = Date.parse("2026-11-06T15:20:00Z");
export const AETHER_CONN = "aether:studio";
export const ARCHON_CONN = "archon:studio";
const ago = (minutes: number): string => new Date(NOW - minutes * 60_000).toISOString();

export function aetherInfo(state: ConnectionInfo["state"] = "ready", features: readonly string[] = ["usage_breakdown", "noesis"]): ConnectionInfo {
  return { id: AETHER_CONN, daemon: "aether", state, protocol: 26, features, standing: "operator", label: "studio.local" };
}

export function archonInfo(state: ConnectionInfo["state"] = "ready"): ConnectionInfo {
  return { id: ARCHON_CONN, daemon: "archon", state, protocol: 24, features: [], standing: "operator", label: "archond" };
}

function usage(cost: number, tokens: number): aether.Usage {
  return { cost_usd: cost, cost_complete: true, input_tokens: tokens, output_tokens: tokens / 4, cache_read_tokens: 0, cache_write_tokens: 0 };
}

function agent(id: string, name: string, status: aether.AgentStatus, model: string, cost: number): aether.Agent {
  return {
    id, name, role: "worker", run: "run_1", model: { provider: "claude-code", model }, status, config: {}, scope: {},
    created_at: ago(600), updated_at: ago(30), last_event_at: ago(5), stats: { turns: 4, tool_calls: 9, usage: usage(cost, 40_000) },
  };
}

export function fixtureAgents(): Record<string, aether.Agent> {
  const list = [
    agent("agt_lead", "lead", { state: "running" }, "opus", 1.2),
    agent("agt_build", "builder", { state: "running" }, "sonnet", 0.4),
    agent("agt_review", "reviewer", { state: "waiting", condition: { on: "message_from", agent: "agt_lead" } } as aether.AgentStatus, "sonnet", 0.2),
    agent("agt_idle", "scout", { state: "ready" }, "haiku", 0.05),
    agent("agt_stuck", "migrator", { state: "blocked", reason: { block: "inspection" } } as aether.AgentStatus, "opus", 0.3),
    agent("agt_broke", "linter", { state: "failed", error: "the engine exited" }, "haiku", 0.01),
    agent("agt_done", "docs", { state: "completed" }, "sonnet", 0.1),
  ];
  return Object.fromEntries(list.map((a) => [a.id, a]));
}

const KINDS = ["agent.started", "agent.turn_completed", "approval.requested", "agent.message", "invocation.completed"];
const ACTORS = ["agt_lead", "agt_build", "agt_review", "agt_stuck"];

/** 30 events over the last ten hours, oldest first as core keeps them; the newest names no agent. */
export function fixtureEvents(): aether.Event[] {
  const events = Array.from({ length: 30 }, (_, i) => ({
    id: `evt_${i + 1}`, seq: i + 1, at: ago((29 - i) * 20 + 1), actor: { kind: "system" },
    kind: { type: KINDS[i % KINDS.length] }, agent_id: i === 29 ? null : ACTORS[i % ACTORS.length],
  }) as unknown as aether.Event);
  invariant(events.every((event, i) => i === 0 || event.seq > (events[i - 1]?.seq ?? 0)), "fixture events are in sequence order");
  return events;
}

export function fixtureAttention(): aether.Attention {
  return {
    last_seq: 30,
    questions: [{ agent: "agt_review", asked_at: ago(12), question: "Ship it?", question_seq: 3 }],
    approvals: [
      { id: "apr_1", agent_id: "agt_build", call_id: "c1", description: "git push", requested_at: ago(4), side_effect: "external", status: { state: "pending" } },
      { id: "apr_0", agent_id: "agt_build", call_id: "c0", description: "rm", requested_at: ago(90), side_effect: "local", status: { state: "resolved" } },
    ] as unknown as aether.ApprovalRequest[],
    agents: [
      { agent: "agt_broke", kind: "agent_failed", reason: "the engine exited", since: ago(40) },
      { agent: "agt_build", kind: "approval", reason: "git push", since: ago(4) },
    ],
  };
}

function item(id: string, title: string, state: archon.QueueStateKind, minutes: number): readonly [string, ArchonItem] {
  const summary = { id, title, state: { state }, priority: "normal", pipeline: "adaptive", updated_at: ago(minutes) };
  return [id, { summary, state, asOf: 1 }] as const;
}

export function fixtureItems(): Record<string, ArchonItem> {
  return Object.fromEntries([
    item("qi_queued", "Upgrade the toolchain", "queued", 3),
    item("qi_running", "Dark mode for the dashboard", "running", 2),
    item("qi_blocked", "Rotate the signing key", "blocked", 50),
    item("qi_done", "Changelog for 1.3", "succeeded", 20),
    item("qi_failed", "Flaky e2e on Windows", "failed", 70),
    item("qi_old", "Retire the old API", "cancelled", 3000),
  ]);
}

export const BREAKDOWN: aether.UsageBreakdown = {
  rows: [
    { key: "claude-code/opus", invocations: 12, usage: usage(1.5, 600_000) },
    { key: "claude-code/sonnet", invocations: 30, usage: usage(0.6, 400_000) },
    { key: "codex/gpt-6-astra", invocations: 3, usage: usage(0.15, 80_000) },
  ],
  total: usage(2.25, 1_080_000),
  truncated: false,
};

export const WATCHER = {
  enabled: true, label_regex: "archon", mention_regex: "@archon", poll_interval_secs: 60, revision: 3,
  repositories: [
    { repository: "acme/site", dispatched: 14, ignored: 3, enabled: true, last_poll_at: ago(2), target: {} },
    { repository: "acme/api", dispatched: 4, ignored: 1, enabled: true, last_poll_at: ago(6), last_error: "rate limited", target: {} },
  ],
} as unknown as archon.GithubWatcherView;

export const QUESTIONS = [{ id: "q1", asked_at: ago(8), options: ["yes", "no"], text: "Merge the release branch?" }];

export const NOESIS_STATUS = { enabled: true, jobs_running: 0, spend_today: { background: usage(0, 0), interactive: usage(0, 0) } };

export const SESSIONS = {
  sessions: [
    { id: "ns_old", agent: "agt_t1", goal: "Git internals", mode: "guided", phase: "practice", created_at: ago(5000), updated_at: ago(4000) },
    { id: "ns_new", agent: "agt_t2", goal: "Rust lifetimes", mode: "guided", phase: "teach", created_at: ago(300), updated_at: ago(10) },
  ],
};

export const LEARNER = {
  concepts: ["demonstrated", "demonstrated", "demonstrated", "exposed", "exposed", "fragile", "misconception", "unseen"]
    .map((state, i) => ({ concept: { id: `c${i}` }, state, strength: 0.5, misconceptions: [], as_of_evidence: 1 })),
};

/**
 * Puts connections into a store as core's syncs would: with `data` every seeded slice, without it
 * only what core reads at ready (an empty attention snapshot and Noesis switched off).
 */
export function seedWorld(store: Store, connections: readonly ConnectionInfo[], data: boolean): void {
  store.update((state) => ({ ...state, connections: Object.fromEntries(connections.map((info) => [info.id, info])) }));
  if (!data) {
    if (!connections.some((info) => info.id === AETHER_CONN && info.state === "ready")) return;
    store.updateAether(AETHER_CONN, (s) => ({ ...s, attention: { last_seq: 0, questions: [], approvals: [], agents: [] }, noesis: { ...NOESIS_STATUS, enabled: false } }));
    return;
  }
  const agents = fixtureAgents();
  store.updateAether(AETHER_CONN, (s) => ({ ...s, agents, roots: Object.keys(agents), events: fixtureEvents(), attention: fixtureAttention(), noesis: NOESIS_STATUS }));
  store.updateArchon(ARCHON_CONN, (s) => ({ ...s, items: fixtureItems() }));
  invariant(Object.keys(store.get().archon[ARCHON_CONN]?.items ?? {}).length === 6, "the store holds the seeded items");
}
