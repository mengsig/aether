// Pure derivations the learning screens render: mastery labels, the chat timeline, the plan's
// order, spend against budget and how a refusal reads. Nothing here holds daemon data.

import { compareStamps, type NoesisCache } from "@aether-ui/core";
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

export type TimelineItem =
  | { readonly kind: "message"; readonly key: string; readonly at: string; readonly role: "learner" | "teacher"; readonly text: string }
  | { readonly kind: "question"; readonly key: string; readonly at: string; readonly question: aether.Question; readonly result: aether.QuestionResult | undefined; readonly open: boolean }
  | { readonly kind: "problem"; readonly key: string; readonly at: string; readonly problem: aether.Problem; readonly conclusion: aether.EventKindNoesisPracticeConcluded | undefined; readonly active: boolean };

/** The prose of a message: its text blocks only; tool calls, results and (sealed) thinking are not prose. */
export function messageText(message: aether.Message): string {
  return message.content.filter((block): block is aether.ContentBlockText => block.type === "text").map((block) => block.text).join("\n\n").trim();
}

/** The chat in time order: transcript prose with the session's questions and problems between it. */
export function buildTimeline(session: aether.NoesisSession, entries: readonly aether.TranscriptEntry[], learn: NoesisCache): TimelineItem[] {
  const items: TimelineItem[] = [];
  for (const entry of entries) {
    const role = entry.message.role === "assistant" ? "teacher" : entry.message.role === "user" ? "learner" : null;
    const text = role === null ? "" : messageText(entry.message);
    if (role !== null && text !== "") items.push({ kind: "message", key: `m${entry.seq}`, at: entry.at, role, text });
  }
  for (const { session: owner, question } of Object.values(learn.questions)) {
    if (owner !== session.id) continue;
    items.push({ kind: "question", key: question.id, at: question.posed_at, question, result: learn.results[question.id], open: session.open_question?.id === question.id });
  }
  for (const { session: owner, problem } of Object.values(learn.problems)) {
    if (owner !== session.id) continue;
    items.push({ kind: "problem", key: problem.id, at: problem.posed_at, problem, conclusion: learn.conclusions[problem.id], active: session.active_problem?.id === problem.id });
  }
  const order = new Map(items.map((item, index) => [item, index]));
  items.sort((a, b) => compareStamps(a.at, b.at) || (order.get(a) ?? 0) - (order.get(b) ?? 0));
  invariant(items.filter((item) => item.kind === "question" && item.open).length <= 1, "a session has at most one open question");
  return items;
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

export type Refusal = { readonly kind: "model"; readonly message: string } | { readonly kind: "other"; readonly message: string };

/** Teaching needs a usable model; the daemon refuses with `allow_api` or `no ready model` when it has none. */
export function classifyRefusal(message: string): Refusal {
  const model = /noesis\.billing\.allow_api|no ready model for noesis|billed per request/i.test(message);
  return { kind: model ? "model" : "other", message };
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

/** A draft key `learn/<part>/…`; ids are reduced to the characters a draft key allows. */
export function draftKey(...parts: readonly string[]): string {
  invariant(parts.length > 0 && parts.every((part) => part.length > 0), "a draft key names its form");
  const key = `learn/${parts.map((part) => part.replace(/[^a-z0-9:_.-]/gi, "-")).join("/")}`;
  invariant(key.length <= 200, "a draft key stays short");
  return key;
}

/** How far a session's plan has come: steps done of all, or null before there is a plan. */
export function planProgress(session: aether.NoesisSession): { readonly done: number; readonly total: number } | null {
  const nodes = session.plan?.nodes ?? [];
  if (nodes.length === 0) return null;
  const done = nodes.filter((node) => node.status === "visited").length;
  invariant(done <= nodes.length, "a plan has no more done steps than steps");
  return { done, total: nodes.length };
}

/** Recent goals, newest first and without repeats, offered as suggestions for a new session. */
export function recentGoals(sessions: readonly aether.NoesisSession[], limit = 3): string[] {
  invariant(limit > 0, "a positive number of suggestions");
  const goals: string[] = [];
  for (const session of sortSessions(sessions)) {
    const goal = session.goal.trim();
    if (goal !== "" && !goals.includes(goal)) goals.push(goal);
    if (goals.length === limit) break;
  }
  return goals;
}
