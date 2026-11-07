// Demo data for the controls and inbox: one agent and the daemon's answers to every read the
// controls make. Shared by the tests and the screenshot page; nothing here is a real credential.

import type { aether } from "@aether-ui/protocol";
import { stamp, type Script } from "./daemon.ts";

export const AGENT_ID = "agt_01J0B3CONTROLS0000000001";
export const CHILD_ID = "agt_01J0B3CONTROLS0000000002";
const RUN = "run_01J0B3CONTROLS000000001";

export function demoAgents(): aether.Agent[] {
  const base = { role: "agent", run: RUN, scope: {}, created_at: stamp(0), updated_at: stamp(1) };
  return [
    {
      ...base, id: AGENT_ID, name: "release-captain", model: { provider: "anthropic", model: "claude-opus-5-5" },
      status: { state: "running" } as aether.AgentStatus,
      config: { effort: "high", cost_budget: 20, max_turns: 200, instructions: "Ship the 1.3 release notes and keep the changelog tidy.", approval: { require_for: ["external"] } },
    },
    {
      ...base, id: CHILD_ID, name: "changelog-writer", parent: AGENT_ID, model: { provider: "openai", model: "gpt-6" },
      status: { state: "waiting" } as aether.AgentStatus, config: {},
    },
  ] as aether.Agent[];
}

const MODELS = [
  { model: { provider: "anthropic", model: "claude-opus-5-5" }, name: "Opus 5.5", provider_name: "Anthropic", configured: true, is_default: true, capabilities: {} },
  { model: { provider: "anthropic", model: "claude-sonnet-5-5" }, name: "Sonnet 5.5", provider_name: "Anthropic", configured: true, is_default: false, capabilities: {} },
  { model: { provider: "local", model: "offline" }, name: "Offline", provider_name: "Local", configured: false, is_default: false, capabilities: {} },
];

const RULES = [
  { id: "rul_1", created_at: stamp(2), created_by: { actor: "operator" }, decision: "approved", match: { tool: "shell", argument: "git status" }, scope: { scope: "agent", id: AGENT_ID } },
  { id: "rul_2", created_at: stamp(3), created_by: { actor: "operator" }, decision: "denied", match: { tool: "http" }, scope: { scope: "everywhere" } },
  { id: "rul_3", created_at: stamp(3), created_by: { actor: "operator" }, decision: "denied", match: { tool: "rm" }, scope: { scope: "agent", id: AGENT_ID }, revoked: { at: stamp(4) } },
];

const SCHEDULES = [
  {
    id: "sch_1", name: "Morning check", created_at: stamp(2), updated_at: stamp(3), deliveries: 12, missed: "coalesce", owner: { actor: "operator" },
    payload: "Check overnight CI", recurrence: { kind: "interval", seconds: 86_400 }, status: { state: "active", next_due: stamp(900) }, target: AGENT_ID,
  },
];

const REPORT = {
  run: RUN, schema: 1, from_seq: 1, to_seq: 40, generated_at: stamp(40),
  body: {
    outcome: "running", objective: "Ship the 1.3 release", started_at: stamp(1), duration_ms: 1_260_000, events: 40,
    usage: { cost_usd: 3.42, cost_complete: true, input_tokens: 1, output_tokens: 1, cache_read_tokens: 0, cache_write_tokens: 0 },
    agents: [], approvals: [], artifacts: [], failures: [], open_loops: [], tasks: [], timeline: [], tools: [],
  },
};

const MUTATIONS = ["interrupt", "pause", "resume", "wake", "compact", "restart", "archive", "cancel", "send_message", "update_agent",
  "set_approvals", "add_approval_rule", "revoke_approval_rule", "update_schedule", "create_schedule", "reply_to_question", "resolve_approval"];

/** Every read the controls make, answered from the demo data, and every change acknowledged; tests override single commands. */
export function controlsScript(): Script {
  const acknowledged = Object.fromEntries(MUTATIONS.map((cmd) => [cmd, () => ({ tag: "ok", data: null })]));
  return {
    ...acknowledged,
    list_models: () => ({ tag: "models", data: MODELS }),
    list_approval_rules: () => ({ tag: "approval_rules", data: RULES }),
    list_schedules: () => ({ tag: "schedules", data: SCHEDULES }),
    schedule_history: () => ({ tag: "schedule_history", data: [{ schedule: "sch_1", due: stamp(10), recorded_at: stamp(11), outcome: { outcome: "delivered", inbox: "inb_1" } }] }),
    list_artifacts: () => ({ tag: "artifacts", data: [{ id: "art_1", name: "release-notes.md", kind: "file", size_bytes: 28, summary: "Draft notes", created_at: stamp(5), created_by: { actor: "system" } }] }),
    get_artifact: (command) => ({ tag: "artifact", data: { id: command.id, name: "release-notes.md", kind: { kind: "file" }, content_type: "text/markdown", size_bytes: 28, summary: "Draft notes", created_at: stamp(5), created_by: { actor: "system" } } }),
    read_artifact: () => ({ tag: "artifact_bytes", data: { base64: btoa("# 1.3\n\n- Faster fleet view\n"), truncated: false } }),
    get_run_report: () => ({ tag: "run_report", data: REPORT }),
    list_runs: () => ({ tag: "runs", data: [{ run: RUN, agents: 2, agents_live: 2, objective: "Ship the 1.3 release", started_at: stamp(1), updated_at: stamp(40), usage: REPORT.body.usage }] }),
    get_inbox: (c) => ({ tag: "inbox", data: [{ id: "inb_1", agent_id: typeof c.agent === "string" ? c.agent : AGENT_ID, seq: 3, delivered_at: stamp(11), kind: { kind: "wake", reason: "schedule" } }] }),
  };
}
