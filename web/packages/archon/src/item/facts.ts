// What an item page states about an item. Execution, pull request and issue are three facts with three
// sources, never merged: a succeeded item can have an open PR, and a delivered PR leaves its issue open.

import type { archon } from "@aether-ui/protocol";
import type { Status } from "@aether-ui/ui";
import { invariant, isTerminal, reasonLabel, statePill, type StateKind } from "../queue/model.ts";

export interface Fact {
  readonly label: string;
  /** Null when there is nothing to colour: no PR, or an issue whose state was never read. */
  readonly status: Status | null;
  readonly text: string;
  readonly detail: string | null;
}

/** Waiting reasons only the operator can clear. */
const OPERATOR_REASONS: ReadonlySet<string> = new Set(["operator_gate", "dependency_failed", "blocked"]);

export function waitsOnYou(kind: StateKind, detail: Pick<archon.ItemDetail, "waiting_on" | "waiting_reason">): boolean {
  if (isTerminal(kind)) return false;
  return Boolean(detail.waiting_on) || kind === "blocked" || OPERATOR_REASONS.has(detail.waiting_reason?.code ?? "");
}

export function executionFact(kind: StateKind, detail: archon.ItemDetail, stage: string | null): Fact {
  invariant(detail.item.id.length > 0, "an execution fact describes an item");
  const pill = statePill(kind, waitsOnYou(kind, detail));
  const reason = isTerminal(kind) ? null : detail.waiting_reason ?? null;
  const where = stage && !isTerminal(kind) ? `stage ${stage}` : null;
  const why = reason ? reason.message || reasonLabel(reason.code) : null;
  return { label: "Execution", status: pill.status, text: pill.label, detail: [where, why].filter(Boolean).join(" · ") || null };
}

const sameRef = (a: archon.GithubRef, b: archon.GithubRef): boolean =>
  a.host === b.host && a.repo.toLowerCase() === b.repo.toLowerCase() && a.number === b.number && a.kind === b.kind;

/** The object GitHub last returned for `ref`, if the links reply carries it. */
export function objectFor(ref: archon.GithubRef | null, objects: readonly archon.GithubObject[]): archon.GithubObject | null {
  if (ref === null) return null;
  return objects.find((object) => sameRef(object.ref, ref)) ?? null;
}

const REVIEW: Readonly<Record<archon.ReviewDecision, string>> = {
  approved: "approved", changes_requested: "changes requested", review_required: "review required",
};

function checksText(checks: archon.ChecksSummary | null | undefined): string | null {
  if (!checks || checks.total === 0) return null;
  if (checks.failed > 0) return `${checks.failed} of ${checks.total} checks failing`;
  return checks.state === "success" ? `${checks.total} checks passed` : `checks ${checks.state}`;
}

/** A fresh GitHub read wins over the summary's PR state, which archond records when it delivers. */
export function prFact(pr: archon.PrRef | null, object: archon.GithubObject | null): Fact {
  if (pr === null && object === null) return { label: "Pull request", status: null, text: "No pull request", detail: null };
  const state: archon.PrState | null = object ? (object.state === "open" && object.draft ? "draft" : object.state) : pr?.state ?? null;
  invariant(state === null || ["open", "draft", "merged", "closed"].includes(state), `a PR state is known, not ${String(state)}`);
  const number = object?.ref.number ?? pr?.number;
  const review = object?.review_decision ?? pr?.review_decision ?? null;
  const detail = [review ? REVIEW[review] : null, checksText(object?.checks ?? pr?.checks)].filter(Boolean).join(" · ") || null;
  if (state === null) return { label: "Pull request", status: null, text: `#${String(number)} · state unknown`, detail };
  return { label: "Pull request", status: `pr-${state}`, text: `#${String(number)} · ${state.charAt(0).toUpperCase()}${state.slice(1)}`, detail };
}

export function issueFact(issue: archon.GithubRef | null, object: archon.GithubObject | null): Fact {
  if (issue === null) return { label: "Issue", status: null, text: "Not linked", detail: null };
  invariant(issue.kind === "issue", `an issue fact reads an issue, not a ${issue.kind}`);
  if (object === null) return { label: "Issue", status: null, text: `#${issue.number} · state not read`, detail: issue.repo };
  const closed = object.state !== "open";
  const reason = closed && object.state_reason ? object.state_reason.replaceAll("_", " ") : null;
  return { label: "Issue", status: closed ? "issue-closed" : "issue-open", text: `#${issue.number} · ${closed ? "Closed" : "Open"}`, detail: [issue.repo, reason].filter(Boolean).join(" · ") };
}

/** The item's issue: the summary's, else an issue link (origin first). */
export function issueOf(summary: archon.ItemSummary | null, links: readonly archon.GithubLink[]): archon.GithubRef | null {
  if (summary?.issue) return summary.issue;
  const issues = links.filter((link) => link.ref.kind === "issue");
  return (issues.find((link) => link.relation === "origin") ?? issues[0])?.ref ?? null;
}

/** The item's PR: the summary's, else the delivered pull request link. */
export function prOf(summary: archon.ItemSummary | null, links: readonly archon.GithubLink[]): archon.PrRef | null {
  if (summary?.pr) return summary.pr;
  const link = links.find((entry) => entry.ref.kind === "pull_request" && entry.relation === "delivered");
  return link ? { ...link.ref } : null;
}

export interface Outcome { readonly tone: "success" | "danger" | "warning" | "neutral"; readonly title: string; readonly text: string }

/** What finished or holds the item, as the retired app's callout said it; null while it simply runs. */
export function outcome(detail: archon.ItemDetail): Outcome | null {
  const state = detail.item.state;
  if (state.state === "failed") return { tone: "danger", title: "Failed", text: state.error };
  if (state.state === "succeeded") return { tone: "success", title: "Done", text: state.summary };
  if (state.state === "cancelled") return { tone: "neutral", title: "Cancelled", text: state.reason ?? "Cancelled by the operator." };
  if (state.state === "blocked") return { tone: "warning", title: "Blocked", text: state.reason };
  const on = detail.waiting_on;
  if (on) return { tone: "warning", title: `Waiting on you · ${on.subject}`, text: on.prompt };
  const reason = detail.waiting_reason;
  if (reason) return { tone: OPERATOR_REASONS.has(reason.code) ? "warning" : "neutral", title: "Waiting", text: reason.message || reasonLabel(reason.code) };
  return null;
}

const RETRYABLE: ReadonlySet<StateKind> = new Set(["failed", "cancelled", "blocked"]);
export const canCancel = (kind: StateKind): boolean => !isTerminal(kind);
export const canRetry = (kind: StateKind): boolean => RETRYABLE.has(kind);

const RUN_PILLS: Readonly<Record<archon.RunStateKind, { status: Status; label: string }>> = {
  running: { status: "running", label: "Running" }, awaiting_operator: { status: "operator", label: "Needs you" },
  succeeded: { status: "succeeded", label: "Succeeded" }, failed: { status: "failed", label: "Failed" }, cancelled: { status: "cancelled", label: "Cancelled" },
};

const STAGE_PILLS: Readonly<Record<archon.StageStateKind, { status: Status; label: string }>> = {
  spawning: { status: "dispatching", label: "Starting" }, running: { status: "running", label: "Running" },
  completed: { status: "succeeded", label: "Completed" }, failed: { status: "failed", label: "Failed" },
  cancelled: { status: "cancelled", label: "Cancelled" }, timed_out: { status: "failed", label: "Timed out" },
};

export function runPill(state: archon.RunState): { status: Status; label: string } {
  const pill = RUN_PILLS[state.state];
  invariant(pill !== undefined, `a run state is known, not ${String(state.state)}`);
  return pill;
}

export function stagePill(state: archon.StageState): { status: Status; label: string } {
  const pill = STAGE_PILLS[state.state];
  invariant(pill !== undefined, `a stage state is known, not ${String(state.state)}`);
  return pill;
}

/** The model a stage ran on last; a switched stage names its final segment. */
export function modelOf(stage: archon.StageRun): string {
  const choice = stage.model_segments?.at(-1)?.choice ?? stage.choice;
  const model = typeof choice.model === "string" ? choice.model : JSON.stringify(choice.model);
  return `${model} · ${choice.effort}`;
}

export function duration(start: string, end: string | null | undefined, now: number): string | null {
  const from = Date.parse(start);
  const to = end ? Date.parse(end) : now;
  if (!Number.isFinite(from) || !Number.isFinite(to) || to < from) return null;
  const minutes = Math.round((to - from) / 60_000);
  if (minutes < 1) return "<1 min";
  return minutes < 60 ? `${minutes} min` : `${Math.floor(minutes / 60)} h ${minutes % 60} min`;
}

export function bytes(count: number): string {
  invariant(Number.isFinite(count) && count >= 0, "a byte count is a non-negative number");
  if (count < 1024) return `${count} B`;
  return count < 1024 * 1024 ? `${(count / 1024).toFixed(1)} KB` : `${(count / 1024 / 1024).toFixed(1)} MB`;
}
