// Pure derivations the learning screens render: mastery labels, the plan's order, spend against
// budget, what billing paid for it and how a refusal reads. Nothing here holds daemon data.

import { compareStamps } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";

export function invariant(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(`Invariant failed: ${message}`);
}

export type Mastery = aether.ConceptStateKind;

/** Each state's text label and what it means; the colour (`--mastery-*`) is never the only signal. */
export const MASTERY: Readonly<Record<Mastery, { readonly label: string; readonly hint: string }>> = {
  misconception: { label: "Misconception", hint: "A wrong idea showed up and is not cleared yet" },
  fragile: { label: "Fragile", hint: "Known once, but slipping or shaky" },
  exposed: { label: "Introduced", hint: "Seen or taught, not yet shown in recall" },
  demonstrated: { label: "Demonstrated", hint: "Recalled correctly on different days" },
  unseen: { label: "Not seen yet", hint: "No evidence yet" },
};

/** Attention first: what needs work leads, what is settled follows. */
export const MASTERY_ORDER: readonly Mastery[] = ["misconception", "fragile", "exposed", "demonstrated", "unseen"];

export function isMastery(value: unknown): value is Mastery {
  return typeof value === "string" && Object.hasOwn(MASTERY, value);
}

export const PHASE_LABEL: Readonly<Record<aether.Phase, string>> = {
  probe: "Finding your starting point", plan: "Planning the path", teach: "Learning", practice: "Practising",
};

export const CONFIDENCE_STOPS = [0, 30, 50, 70, 90, 100] as const;
export type Confidence = (typeof CONFIDENCE_STOPS)[number];

export const OUTCOME_LABEL: Readonly<Record<aether.PracticeOutcome, string>> = {
  solved: "Solved", solved_with_hints: "Solved with hints", moved_on: "Moved on", surrendered: "Shown the answer", dismissed: "Set aside",
};

/** The prose of a message: its text blocks only; tool calls, results and (sealed) thinking are not prose. */
export function messageText(message: aether.Message): string {
  return message.content.filter((block): block is aether.ContentBlockText => block.type === "text").map((block) => block.text).join("\n\n").trim();
}

export interface PlanRow {
  readonly node: aether.PlanNode;
  /** The longest chain of prerequisites before it, so a step sits below everything it needs. */
  readonly depth: number;
}

/** The plan's steps in prerequisite order (Kahn, stable by the plan's own order); a cycle's leftovers come last. */
export function planRows(plan: aether.PlanTree): PlanRow[] {
  const ids = new Set(plan.nodes.map((node) => node.id));
  invariant(ids.size === plan.nodes.length, "plan node ids are unique");
  const edges = plan.edges.filter((edge) => ids.has(edge.from) && ids.has(edge.to) && edge.from !== edge.to);
  const incoming = new Map(plan.nodes.map((node) => [node.id, 0]));
  for (const edge of edges) incoming.set(edge.to, (incoming.get(edge.to) ?? 0) + 1);
  const depth = new Map(plan.nodes.map((node) => [node.id, 0]));
  const placed: PlanRow[] = [];
  const done = new Set<string>();
  let ready = plan.nodes.filter((node) => incoming.get(node.id) === 0);
  while (ready.length > 0) {
    const [node, ...rest] = ready as [aether.PlanNode, ...aether.PlanNode[]];
    ready = rest;
    done.add(node.id);
    placed.push({ node, depth: depth.get(node.id) ?? 0 });
    for (const edge of edges.filter((e) => e.from === node.id)) {
      depth.set(edge.to, Math.max(depth.get(edge.to) ?? 0, (depth.get(node.id) ?? 0) + 1));
      const left = (incoming.get(edge.to) ?? 0) - 1;
      incoming.set(edge.to, left);
      if (left === 0) ready = [...ready, ...plan.nodes.filter((n) => n.id === edge.to)];
    }
  }
  for (const node of plan.nodes) if (!done.has(node.id)) placed.push({ node, depth: 0 });
  invariant(placed.length === plan.nodes.length, "every plan node is placed once");
  return placed;
}

export const PLAN_STATUS_LABEL: Readonly<Record<aether.PlanNodeStatus, string>> = { visited: "Done", current: "Now", upcoming: "Later" };

/** What a day's work cost in dollars, estimates for subscription engines included, as the daemon's budget counts it. */
export function spentUsd(usage: aether.Usage | undefined): number {
  if (usage === undefined) return 0;
  const spent = usage.cost_usd + (usage.estimated_cost_usd ?? 0);
  invariant(Number.isFinite(spent) && spent >= 0, "spend is a nonnegative amount");
  return spent;
}

/** The share of a budget spent, 0..1; a zero budget refuses everything, so it reads as full. */
export function budgetShare(spent: number, budget: number): number {
  invariant(spent >= 0 && budget >= 0, "spend and budget are nonnegative");
  if (budget === 0) return 1;
  return Math.min(1, spent / budget);
}

export const usd = (value: number): string => `$${value.toFixed(value >= 100 ? 0 : 2)}`;

const tokensOf = (usage: aether.Usage): number => usage.input_tokens + usage.output_tokens + usage.cache_read_tokens + usage.cache_write_tokens;

type BillingSplit = { readonly subscription?: aether.Usage; readonly api?: aether.Usage; readonly unknown?: aether.Usage };

const used = (usage: aether.Usage | undefined): boolean => usage !== undefined && (tokensOf(usage) > 0 || spentUsd(usage) > 0);

/**
 * What paid for some model use, in words: subscription use is covered by the plan (its dollars are a
 * list-price estimate), API use is billed per request. Subscription use never reads as a charge, and
 * use on a provider no longer configured is named as unknown rather than guessed.
 */
export function billingWords(usage: BillingSplit | null | undefined): string {
  if (usage == null) return "No model use recorded yet.";
  // A daemon from before the billing split reports neither half: say so rather than claim no use.
  if (usage.api === undefined && usage.subscription === undefined) return "This daemon does not report what paid for its model use.";
  const covered = `no per-request charge (≈${usd(spentUsd(usage.subscription))} at API list price)`;
  const parts: string[] = [];
  if (used(usage.api)) parts.push(`${usd(spentUsd(usage.api))} billed to your API key`);
  if (used(usage.subscription)) parts.push(parts.length > 0 ? `the rest ran on your subscription — ${covered}` : `Ran on your subscription — ${covered}`);
  const unknown = "ran on a provider no longer configured, so what paid for it is unknown";
  if (used(usage.unknown)) parts.push(parts.length > 0 ? `some ${unknown}` : `Some ${unknown}`);
  return parts.length === 0 ? "No model use recorded yet." : `${parts.join("; ")}.`;
}

/** Every step of the plan is done: the lesson is complete. */
export function planComplete(plan: aether.PlanTree | null | undefined): boolean {
  if (plan == null || plan.nodes.length === 0) return false;
  invariant(new Set(plan.nodes.map((node) => node.id)).size === plan.nodes.length, "plan node ids are unique");
  return plan.nodes.every((node) => node.status === "visited");
}

export type Refusal = { readonly kind: "model" | "budget" | "other"; readonly message: string };

/**
 * Teaching needs a usable model (without one the daemon names `allow_api` or `no ready model`) and
 * budget left (it names `noesis.budget.interactive_usd_daily`, or says the day's budget is spent).
 */
export function classifyRefusal(message: string): Refusal {
  invariant(message.trim() !== "", "a refusal says why");
  if (/noesis\.budget\.interactive_usd_daily|interactive budget of \$[\d.]+ is spent/i.test(message)) return { kind: "budget", message };
  const model = /noesis\.billing\.allow_api|no ready model for noesis|billed per request/i.test(message);
  return { kind: model ? "model" : "other", message };
}

export interface BudgetUse {
  readonly spent: number;
  readonly limit: number;
}

/** Today's teaching spend against its budget once it is used up: the daemon then refuses every turn. */
export function exhaustedBudget(status: aether.NoesisStatus): BudgetUse | null {
  const limit = status.settings.budget.interactive_usd_daily;
  const spent = spentUsd(status.spend_today.interactive);
  invariant(Number.isFinite(limit) && limit >= 0, "the teaching budget is a nonnegative amount");
  return spent >= limit ? { spent, limit } : null;
}

/** Sessions, newest activity first. */
export function sortSessions(sessions: readonly aether.NoesisSession[]): aether.NoesisSession[] {
  return [...sessions].sort((a, b) => compareStamps(b.updated_at, a.updated_at) || a.id.localeCompare(b.id));
}

/** What the learner should do next in a session, in a few words. */
export function nextStep(session: aether.NoesisSession): string {
  if (session.open_question) return "A question is waiting for you";
  if (session.active_problem) return "A problem is waiting for you";
  if (session.phase === "plan") return "Review the plan";
  return "Continue the conversation";
}

export const dateTime = (stamp: string): string =>
  new Date(stamp).toLocaleString("en", { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
