// The agent detail's facts, derived from the agent row core keeps: what it is, where it runs,
// what it has cost and why it stopped. Nothing here reads a daemon.

import type { aether } from "@aether-ui/protocol";
import { invariant } from "../fleet/invariant.ts";
import { costCaveat, formatCost, formatTime, formatTokens } from "../conversation/format.ts";

export interface Fact {
  readonly label: string;
  readonly value: string;
  readonly hint?: string;
}

/** Why the agent is in its state, when the state carries a reason. */
export function statusDetail(status: aether.AgentStatus): string | null {
  switch (status.state) {
    case "failed": return status.error;
    case "blocked": return "detail" in status.reason && typeof status.reason.detail === "string" ? status.reason.detail : `Blocked on ${status.reason.block}`;
    case "cancelled": return status.reason ?? null;
    case "waiting": return `Waiting on ${status.condition.on.replaceAll("_", " ")}`;
    default: return null;
  }
}

export function usageFacts(usage: aether.Usage | undefined): Fact[] {
  if (!usage) return [{ label: "Cost", value: "Nothing spent yet" }];
  const caveat = costCaveat(usage);
  return [
    { label: "Cost", value: formatCost(usage), ...(caveat ? { hint: caveat } : {}) },
    { label: "Tokens", value: `${formatTokens(usage.input_tokens)} in · ${formatTokens(usage.output_tokens)} out` },
    { label: "Cache", value: `${formatTokens(usage.cache_read_tokens)} read · ${formatTokens(usage.cache_write_tokens)} written` },
    ...(usage.reasoning_tokens ? [{ label: "Reasoning", value: `${formatTokens(usage.reasoning_tokens)} tokens` }] : []),
  ];
}

function budgetFact(config: aether.AgentConfig): Fact | null {
  const parts: string[] = [];
  if (config.cost_budget != null) parts.push(`$${config.cost_budget.toFixed(2)}`);
  if (config.token_budget != null) parts.push(`${formatTokens(config.token_budget)} tokens`);
  return parts.length === 0 ? null : { label: "Budget", value: parts.join(" · ") };
}

/** The facts grid, in reading order; optional facts appear only when the agent has them. */
export function agentFacts(agent: aether.Agent): Fact[] {
  invariant(agent.id !== "" && agent.model.model !== "", `agent ${agent.id} has an id and a model`);
  const stats = agent.stats ?? {};
  const facts: Fact[] = [
    { label: "Model", value: `${agent.model.provider}/${agent.model.model}`, ...(agent.config.effort ? { hint: `effort ${agent.config.effort}` } : {}) },
    { label: "Run", value: agent.run },
    { label: "Created", value: formatTime(agent.created_at) },
  ];
  if (agent.last_event_at) facts.push({ label: "Last activity", value: formatTime(agent.last_event_at) });
  facts.push({ label: "Turns", value: String(stats.turns ?? 0) }, { label: "Tool calls", value: String(stats.tool_calls ?? 0) });
  if (stats.context_tokens != null) facts.push({ label: "Context", value: `${formatTokens(stats.context_tokens)} tokens` });
  facts.push(...usageFacts(stats.usage));
  const budget = budgetFact(agent.config);
  if (budget) facts.push(budget);
  invariant(new Set(facts.map((fact) => fact.label)).size === facts.length, "each fact appears once");
  return facts;
}
