// Bundles from Archon reads: an item's detail, one run of it with its plan, and a pull request as
// `list_github_objects` returned it. Pure: the sources in sources.ts pass in what core stored.

import type { aether, archon } from "@aether-ui/protocol";
import { invariant } from "../learn/model.ts";
import { BRIEF_MAX_BYTES, bundleOf, evidence } from "./bundle.ts";

// Status rules. verified: only a concrete check or merge record (a passed test stage, a shipped run
// or item, a merged PR); reviewed: a passed review stage or approved gate; everything else, a run
// that merely succeeded included, is preliminary. Each status carries the record behind it.

export interface Standing {
  readonly status: aether.EvidenceStatus;
  /** The record the status rests on, in words shown with the evidence. */
  readonly basis: string;
}

const passed = (stage: archon.StageRun, kind: archon.StageKind): boolean =>
  stage.kind === kind && stage.state.state === "completed" && stage.verdict === "pass";

export function stageStanding(stage: archon.StageRun): Standing {
  if (passed(stage, "test")) return { status: "verified", basis: "Verified: this test stage passed." };
  if (passed(stage, "ship")) return { status: "verified", basis: "Verified: this ship stage passed." };
  if (passed(stage, "review")) return { status: "reviewed", basis: "Reviewed: this review stage passed." };
  return { status: "preliminary", basis: "Preliminary: no check or review passed this stage." };
}

/** A run is verified by a merge or a passing test stage of its own, never by succeeding alone. */
export function runStanding(run: archon.PipelineRun, stages: readonly archon.StageRun[]): Standing {
  invariant(stages.every((stage) => stage.run === run.id), `the stages judged are run ${run.id}'s`);
  if (run.shipped === true) return { status: "verified", basis: "Verified: the run shipped its change." };
  const test = stages.find((stage) => passed(stage, "test"));
  if (test !== undefined) return { status: "verified", basis: `Verified: test stage ${test.stage_id} passed.` };
  const review = stages.find((stage) => passed(stage, "review"));
  if (review !== undefined) return { status: "reviewed", basis: `Reviewed: review stage ${review.stage_id} passed.` };
  const ended = run.state.state === "succeeded" ? "succeeded, but" : "is not finished, and";
  return { status: "preliminary", basis: `Preliminary: the run ${ended} no test, review or merge confirmed it.` };
}

export function gateStanding(gate: archon.OperatorGate): Standing {
  if (gate.decision?.decision === "approve" && !gate.withdrawn) return { status: "reviewed", basis: "Reviewed: the operator approved this gate." };
  return { status: "preliminary", basis: "Preliminary: the operator has not approved this gate." };
}

export function itemStanding(detail: archon.ItemDetail): Standing {
  if (detail.runs.some((run) => run.shipped === true)) return { status: "verified", basis: "Verified: a run of this item shipped." };
  return { status: "preliminary", basis: "Preliminary: no run of this item has shipped." };
}

export function prStanding(state: archon.PrState | archon.GithubObjectState | null | undefined, review: archon.ReviewDecision | null | undefined): Standing {
  if (state === "merged") return { status: "verified", basis: "Verified: the pull request is merged." };
  if (review === "approved") return { status: "reviewed", basis: "Reviewed: a reviewer approved the pull request." };
  return { status: "preliminary", basis: "Preliminary: the pull request is not merged or approved." };
}

/** Evidence whose excerpt ends with the record behind its status, so the learner and Noesis both see it. */
export function attested(ref: aether.EvidenceRef, standing: Standing, text: string, max?: number): aether.EvidenceItem {
  invariant(standing.basis.length > 0, "a status names its basis");
  return evidence(ref, standing.status, `${text}\n${standing.basis}`, max);
}

function runWords(run: archon.PipelineRun): string {
  const state = run.state;
  switch (state.state) {
    case "running": return `running stage ${state.stage_id} (attempt ${state.attempt})`;
    case "awaiting_operator": return `waiting for the operator at ${state.stage_id}`;
    case "succeeded": return `succeeded${run.shipped ? " and shipped" : ""}: ${state.summary}`;
    case "failed": return `failed at ${state.stage_id}: ${state.error}`;
    case "cancelled": return `cancelled${state.reason ? `: ${state.reason}` : ""}`;
  }
}

function stageWords(stage: archon.StageRun): string {
  const state = stage.state;
  const verdict = stage.verdict && stage.verdict !== "unknown" ? `, verdict ${stage.verdict}` : "";
  const failed = state.state === "failed" ? `: ${state.error}` : "";
  const head = `Stage ${stage.stage_id} (${stage.kind ?? "work"}, attempt ${stage.attempt}): ${state.state.replace("_", " ")}${verdict}${failed}`;
  return stage.summary ? `${head}\n${stage.summary}` : head;
}

function itemState(state: archon.QueueState): string {
  switch (state.state) {
    case "succeeded": return `succeeded: ${state.summary}`;
    case "failed": return `failed: ${state.error}`;
    case "cancelled": return `cancelled${state.reason ? `: ${state.reason}` : ""}`;
    default: return state.state;
  }
}

/** The newest attempt of each stage of one run, in the order the stages first started. */
export function latestStages(stages: readonly archon.StageRun[], run: string): archon.StageRun[] {
  invariant(run.length > 0, "stages are picked for a run");
  const newest = new Map<string, archon.StageRun>();
  for (const stage of stages) {
    if (stage.run !== run) continue;
    const held = newest.get(stage.stage_id);
    if (held === undefined || stage.attempt >= held.attempt) newest.set(stage.stage_id, stage);
  }
  const picked = [...newest.values()];
  invariant(new Set(picked.map((stage) => stage.stage_id)).size === picked.length, "one attempt per stage");
  return picked;
}

function prEvidence(pr: archon.PrRef, url: string | null): aether.EvidenceItem {
  const state = pr.state ?? "unknown";
  const review = pr.review_decision ? `, review ${pr.review_decision.replace("_", " ")}` : "";
  return attested({ kind: "pr", id: `${pr.repo}#${pr.number}`, url }, prStanding(pr.state, pr.review_decision), `Pull request #${pr.number} in ${pr.repo}: ${state}${review}`);
}

function gateEvidence(gate: archon.OperatorGate): aether.EvidenceItem {
  const decision = gate.decision ? `${gate.decision.decision}d` : gate.withdrawn ? "withdrawn" : "awaiting the operator";
  return attested({ kind: "stage", id: `${gate.run}/${gate.stage_id}` }, gateStanding(gate), `Operator gate on ${gate.stage_id} (${gate.subject}): ${decision}\n${gate.summary}`);
}

/** An item: its state, brief, PR, every run in a line, the newest run's stages and its gates. */
export function itemBundle(detail: archon.ItemDetail, summary: archon.ItemSummary | null): aether.EvidenceBundle {
  const item = detail.item;
  invariant(item.id.length > 0 && item.title.trim().length > 0, "an item bundle reads a whole item");
  invariant(summary === null || summary.id === item.id, `the summary of ${item.id} is that item's`);
  const ref = { kind: "item", id: item.id } as const;
  const items: aether.EvidenceItem[] = [
    attested(ref, itemStanding(detail), `${item.title}\nState: ${itemState(item.state)} · pipeline ${detail.switched_to ?? item.pipeline}`),
    evidence(ref, "preliminary", `Brief:\n${item.brief}`, BRIEF_MAX_BYTES),
  ];
  if (summary?.pr) items.push(prEvidence(summary.pr, summary.pr_url ?? null));
  detail.runs.forEach((run, index) => items.push(attested({ kind: "run", id: run.id }, runStanding(run, latestStages(detail.stages, run.id)), `Run ${index + 1} (${run.pipeline}, branch ${run.branch}): ${runWords(run)}`)));
  const newest = detail.runs.at(-1);
  if (newest !== undefined) {
    for (const stage of latestStages(detail.stages, newest.id)) items.push(attested({ kind: "stage", id: stage.id }, stageStanding(stage), stageWords(stage)));
    for (const gate of detail.gates ?? []) if (gate.run === newest.id) items.push(gateEvidence(gate));
  }
  return bundleOf(item.title, items);
}

/** One run: its state and plan, the item it serves, its stages' newest attempts and its gates. */
export function runBundle(detail: archon.ItemDetail, runId: string, plan: archon.RunPlan | null): aether.EvidenceBundle {
  const index = detail.runs.findIndex((entry) => entry.id === runId);
  const run = detail.runs[index];
  invariant(run !== undefined, `run ${runId} is one of ${detail.item.id}'s runs`);
  invariant(plan === null || plan.run === runId, `the plan read is run ${runId}'s`);
  const planned = plan ? `\nPlanned stages: ${plan.stages.map((stage) => stage.id).join(" → ")}` : "";
  const stages = latestStages(detail.stages, run.id);
  const items: aether.EvidenceItem[] = [
    attested({ kind: "run", id: run.id }, runStanding(run, stages), `Run ${index + 1} of "${detail.item.title}" (${run.pipeline}, branch ${run.branch}): ${runWords(run)}${planned}`),
    evidence({ kind: "item", id: detail.item.id }, "preliminary", `Brief of the item:\n${detail.item.brief}`, BRIEF_MAX_BYTES),
  ];
  for (const stage of stages) items.push(attested({ kind: "stage", id: stage.id }, stageStanding(stage), stageWords(stage)));
  for (const gate of detail.gates ?? []) if (gate.run === run.id) items.push(gateEvidence(gate));
  return bundleOf(`Run ${index + 1} of ${detail.item.title}`, items);
}
