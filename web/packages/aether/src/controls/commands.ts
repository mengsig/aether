// Pure builders for the agent controls: each turns a form's draft into one protocol command, and
// throws a plain message for input a person can fix. The screen shows that message beside the form.

import { aether } from "@aether-ui/protocol";
import { invariant } from "../fleet/invariant.ts";

export const EFFORTS: readonly aether.Effort[] = ["low", "medium", "high", "x_high", "max"];
export const SIDE_EFFECTS: readonly aether.SideEffectClass[] = ["read_only", "idempotent_mutation", "mutation", "external"];

/** Input a person typed that cannot be sent as it is; its message is shown, never thrown past the form. */
export class DraftError extends Error {}

function need(condition: boolean, message: string): asserts condition {
  if (!condition) throw new DraftError(message);
}

/** `provider/model`, as the model list and the agent header write it. */
export function modelRef(text: string): aether.ModelRef {
  const value = text.trim();
  const slash = value.indexOf("/");
  need(slash > 0 && slash < value.length - 1, "Choose a model as provider/model");
  const ref = { provider: value.slice(0, slash), model: value.slice(slash + 1) };
  invariant(ref.provider !== "" && ref.model !== "", "both halves of a model reference are named");
  return ref;
}

/** A blank field leaves the budget alone; a number must be positive. */
function optionalPositive(text: string, label: string, whole: boolean): number | null {
  const value = text.trim();
  if (value === "") return null;
  const number = Number(value);
  need(Number.isFinite(number) && number > 0, `${label} must be a positive number`);
  need(!whole || Number.isSafeInteger(number), `${label} must be a whole number`);
  return number;
}

export interface BudgetDraft { readonly cost: string; readonly tokens: string; readonly turns: string }

export function budgetPatch(draft: BudgetDraft): aether.AgentPatch {
  const patch: aether.AgentPatch = {};
  const cost = optionalPositive(draft.cost, "The cost budget", false);
  const tokens = optionalPositive(draft.tokens, "The token budget", true);
  const turns = optionalPositive(draft.turns, "The turn limit", true);
  if (cost !== null) patch.cost_budget = cost;
  if (tokens !== null) patch.token_budget = tokens;
  if (turns !== null) patch.max_turns = turns;
  need(Object.keys(patch).length > 0, "Set at least one limit");
  return patch;
}

/** A message to `agent`, trimmed unless `verbatim` (a `/` line goes to the agent byte for byte). */
export function sendCommand(agent: string, content: string, subtree: boolean, verbatim = false): aether.SendMessageCommand {
  invariant(agent !== "", "a message names its agent");
  need(content.trim() !== "", "Write a message first");
  return aether.sendMessage({ to: agent, content: verbatim ? content : content.trim(), ...(subtree ? { subtree: true } : {}) });
}

export interface RuleDraft {
  readonly tool: string;
  readonly argument: string;
  readonly resource: string;
  readonly note: string;
  readonly decision: aether.ApprovalDecision;
  readonly scope: "agent" | "subtree" | "run";
  readonly run?: string | null;
}

export function ruleCommand(agent: string, draft: RuleDraft): aether.AddApprovalRuleCommand {
  invariant(agent.trim() !== "", "a rule names its agent");
  invariant(draft.decision === "approved" || draft.decision === "denied", "a rule approves or denies matching calls");
  invariant(["agent", "subtree", "run"].includes(draft.scope), "browser rules target an agent, subtree or run");
  const run = draft.run?.trim() ?? "";
  need(draft.scope !== "run" || run !== "", "This agent has no run to apply a rule to");
  const scope: aether.RuleScope = draft.scope === "run" ? { scope: "run", id: run }
    : draft.scope === "subtree" ? { scope: "subtree", root: agent } : { scope: "agent", id: agent };
  const tool = draft.tool.trim();
  const argument = draft.argument.trim();
  const resource = draft.resource.trim();
  const note = draft.note.trim();
  return aether.addApprovalRule({ scope, decision: draft.decision, ...(note ? { note } : {}),
    match: { ...(tool ? { tool } : {}), ...(argument ? { argument } : {}), ...(resource ? { resource_prefix: resource } : {}) } });
}
