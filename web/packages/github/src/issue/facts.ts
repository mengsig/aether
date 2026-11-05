// What a GitHub page states. An item's execution ("succeeded"), a pull request's "merged" and an issue's or
// PR's "closed" come from different sources and are three different facts: none is ever shown as another.

import type { archon } from "@aether-ui/protocol";
import type { Status } from "@aether-ui/ui";
import { invariant } from "./ref.ts";

export interface StateFact {
  readonly label: string;
  /** Null when there is nothing to colour: the state was never read, or is not one this side has. */
  readonly status: Status | null;
  readonly text: string;
  readonly detail: string | null;
}

export const EXECUTION_LABELS: Readonly<Record<archon.QueueStateKind, string>> = {
  queued: "Queued", dispatching: "Starting", running: "Running", blocked: "Blocked", succeeded: "Succeeded", failed: "Failed", cancelled: "Cancelled",
};

export const RUN_LABELS: Readonly<Record<archon.RunStateKind, { readonly status: Status; readonly label: string }>> = {
  running: { status: "running", label: "Running" }, awaiting_operator: { status: "operator", label: "Needs you" },
  succeeded: { status: "succeeded", label: "Succeeded" }, failed: { status: "failed", label: "Failed" }, cancelled: { status: "cancelled", label: "Cancelled" },
};

const REVIEW: Readonly<Record<archon.ReviewDecision, string>> = {
  approved: "Approved", changes_requested: "Changes requested", review_required: "Review required",
};

/** An item's execution, read from its queue state only: never from its issue or pull request. */
export function executionFact(kind: string, stage: string | null): StateFact {
  const label = EXECUTION_LABELS[kind as archon.QueueStateKind];
  if (label === undefined) return { label: "Execution", status: null, text: `Unknown state ${kind}`, detail: null };
  const live = kind === "running" || kind === "dispatching";
  return { label: "Execution", status: kind as archon.QueueStateKind, text: label, detail: live && stage ? `stage ${stage}` : null };
}

export function reviewText(decision: archon.ReviewDecision | null | undefined): string | null {
  return decision ? REVIEW[decision] ?? null : null;
}

export function checksText(checks: archon.ChecksSummary | null | undefined): string | null {
  if (!checks || checks.total === 0) return null;
  invariant(checks.failed >= 0 && checks.failed <= checks.total, `failed checks ${checks.failed} lie within the ${checks.total} run`);
  if (checks.failed > 0) return `${checks.failed} of ${checks.total} checks failing`;
  return checks.state === "success" ? `${checks.total} checks passed` : `Checks ${checks.state}`;
}

/** A pull request's state: GitHub's word, a draft told apart from an open PR. */
export function prState(object: archon.GithubObject | null, recorded: archon.PrState | null | undefined): archon.PrState | null {
  if (object === null) return recorded ?? null;
  invariant(object.ref.kind === "pull_request", `a PR state reads a pull request, not a ${object.ref.kind}`);
  return object.state === "open" && object.draft ? "draft" : object.state;
}

const PR_WORDS: Readonly<Record<archon.PrState, string>> = { open: "Open", draft: "Draft", merged: "Merged", closed: "Closed" };

/** A pull request as GitHub last said, else as Archon recorded it when it delivered. */
export function pullRequestFact(label: string, pr: archon.PrRef | null, object: archon.GithubObject | null): StateFact {
  const number = object?.ref.number ?? pr?.number;
  invariant(number !== undefined, "a pull request fact names a pull request");
  const state = prState(object, pr?.state);
  const detail = [reviewText(object?.review_decision ?? pr?.review_decision), checksText(object?.checks ?? pr?.checks)].filter(Boolean).join(" · ") || null;
  if (state === null) return { label, status: null, text: `#${number} · state unknown`, detail };
  invariant(PR_WORDS[state] !== undefined, `a PR state is known, not ${String(state)}`);
  return { label, status: `pr-${state}`, text: `#${number} · ${PR_WORDS[state]}`, detail };
}

export function issueFact(object: archon.GithubObject): StateFact {
  invariant(object.ref.kind === "issue", `an issue fact reads an issue, not a ${object.ref.kind}`);
  const reason = object.state_reason ? object.state_reason.replaceAll("_", " ") : null;
  if (object.state === "open") return { label: "Issue", status: "issue-open", text: "Open", detail: null };
  // An issue is never merged; a daemon that says so is shown, not coloured as a PR or as closed.
  if (object.state !== "closed") return { label: "Issue", status: null, text: `State ${object.state} is not an issue state`, detail: null };
  return { label: "Issue", status: "issue-closed", text: "Closed", detail: reason };
}

/** The page's own object on GitHub: what Archon's cache last read, or why it has nothing. */
export function githubFact(kind: archon.GithubKind | null, object: archon.GithubObject | null, recorded: archon.PrRef | null): StateFact {
  invariant(object === null || kind === object.ref.kind, "the page's kind is its object's kind");
  if (kind === "pull_request" && (object !== null || recorded !== null)) {
    const fact = pullRequestFact("Pull request", recorded, object);
    return { ...fact, text: fact.text.replace(/^#\d+ · /, "") };
  }
  if (object !== null) return issueFact(object);
  const label = kind === "issue" ? "Issue" : kind === "pull_request" ? "Pull request" : "On GitHub";
  return { label, status: null, text: "Not read from GitHub yet", detail: null };
}

/** What finished or holds an item, in Archon's words; null while it simply runs. */
export function outcomeText(state: archon.QueueState | null): string | null {
  if (state === null) return null;
  switch (state.state) {
    case "succeeded": return state.summary || null;
    case "failed": return state.error || null;
    case "cancelled": return state.reason ?? "Cancelled by the operator.";
    case "blocked": return state.reason || null;
    default: return null;
  }
}
