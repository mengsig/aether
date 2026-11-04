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

export function sendCommand(agent: string, content: string, subtree: boolean): aether.SendMessageCommand {
  invariant(agent !== "", "a message names its agent");
  need(content.trim() !== "", "Write a message first");
  return aether.sendMessage({ to: agent, content: content.trim(), ...(subtree ? { subtree: true } : {}) });
}

export function ruleCommand(agent: string, draft: { tool: string; argument: string; decision: aether.ApprovalDecision; subtree: boolean }): aether.AddApprovalRuleCommand {
  invariant(agent !== "", "a rule names its agent");
  const tool = draft.tool.trim();
  need(tool !== "", "Name the tool the rule matches");
  const scope = (draft.subtree ? { scope: "subtree", root: agent } : { scope: "agent", id: agent }) as aether.RuleScope;
  const argument = draft.argument.trim();
  return aether.addApprovalRule({ scope, decision: draft.decision, match: { tool, ...(argument ? { argument } : {}) } });
}

export interface ScheduleDraft { readonly name: string; readonly seconds: string; readonly message: string }

export function scheduleCommand(agent: string, draft: ScheduleDraft): aether.CreateScheduleCommand {
  invariant(agent !== "", "a schedule names its target");
  need(draft.name.trim() !== "", "Name the schedule");
  const seconds = optionalPositive(draft.seconds, "The interval", true);
  need(seconds !== null && seconds >= 60, "The interval is at least 60 seconds");
  need(draft.message.trim() !== "", "Write the message the schedule delivers");
  const recurrence = { kind: "interval", seconds } as aether.Recurrence;
  return aether.createSchedule({ spec: { name: draft.name.trim(), target: agent, recurrence, payload: draft.message.trim(), busy: "skip", missed: "coalesce" } });
}
