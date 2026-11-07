// Bundles from Archon reads: an item's detail, one run of it with its plan, and a pull request as
// `list_github_objects` returned it. Pure: the sources in sources.ts pass in what core stored.

import type { aether, archon } from "@aether-ui/protocol";
import { invariant } from "../learn/model.ts";
import { BRIEF_MAX_BYTES, bundleOf, evidence } from "./bundle.ts";

// Status rules. verified: only what a check or merge established (a merged PR, passing checks, a
// succeeded run, a stage that passed, a shipped item); reviewed: a passed review or approved gate;
// everything else (briefs, agent output, running or failed work, open PRs) is preliminary.

export function runStatus(run: archon.PipelineRun): aether.EvidenceStatus {
  return run.state.state === "succeeded" ? "verified" : "preliminary";
}

export function stageStatus(stage: archon.StageRun): aether.EvidenceStatus {
  if (stage.state.state !== "completed" || stage.verdict !== "pass") return "preliminary";
  return stage.kind === "review" ? "reviewed" : "verified";
}

export function gateStatus(gate: archon.OperatorGate): aether.EvidenceStatus {
  return gate.decision?.decision === "approve" && !gate.withdrawn ? "reviewed" : "preliminary";
}

export function itemStatus(detail: archon.ItemDetail): aether.EvidenceStatus {
  return detail.runs.some((run) => run.shipped === true) ? "verified" : "preliminary";
}

export function prStatus(state: archon.PrState | archon.GithubObjectState | null | undefined, review: archon.ReviewDecision | null | undefined): aether.EvidenceStatus {
  if (state === "merged") return "verified";
  return review === "approved" ? "reviewed" : "preliminary";
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
  return evidence({ kind: "pr", id: `${pr.repo}#${pr.number}`, url }, prStatus(pr.state, pr.review_decision), `Pull request #${pr.number} in ${pr.repo}: ${state}${review}`);
}

function gateEvidence(gate: archon.OperatorGate): aether.EvidenceItem {
  const decision = gate.decision ? `${gate.decision.decision}d` : gate.withdrawn ? "withdrawn" : "awaiting the operator";
  return evidence({ kind: "stage", id: `${gate.run}/${gate.stage_id}` }, gateStatus(gate), `Operator gate on ${gate.stage_id} (${gate.subject}): ${decision}\n${gate.summary}`);
}

/** An item: its state, brief, PR, every run in a line, the newest run's stages and its gates. */
export function itemBundle(detail: archon.ItemDetail, summary: archon.ItemSummary | null): aether.EvidenceBundle {
  const item = detail.item;
  invariant(item.id.length > 0 && item.title.trim().length > 0, "an item bundle reads a whole item");
  invariant(summary === null || summary.id === item.id, `the summary of ${item.id} is that item's`);
  const ref = { kind: "item", id: item.id } as const;
  const items: aether.EvidenceItem[] = [
    evidence(ref, itemStatus(detail), `${item.title}\nState: ${itemState(item.state)} · pipeline ${detail.switched_to ?? item.pipeline}`),
    evidence(ref, "preliminary", `Brief:\n${item.brief}`, BRIEF_MAX_BYTES),
  ];
  if (summary?.pr) items.push(prEvidence(summary.pr, summary.pr_url ?? null));
  detail.runs.forEach((run, index) => items.push(evidence({ kind: "run", id: run.id }, runStatus(run), `Run ${index + 1} (${run.pipeline}, branch ${run.branch}): ${runWords(run)}`)));
  const newest = detail.runs.at(-1);
  if (newest !== undefined) {
    for (const stage of latestStages(detail.stages, newest.id)) items.push(evidence({ kind: "stage", id: stage.id }, stageStatus(stage), stageWords(stage)));
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
  const items: aether.EvidenceItem[] = [
    evidence({ kind: "run", id: run.id }, runStatus(run), `Run ${index + 1} of "${detail.item.title}" (${run.pipeline}, branch ${run.branch}): ${runWords(run)}${planned}`),
    evidence({ kind: "item", id: detail.item.id }, "preliminary", `Brief of the item:\n${detail.item.brief}`, BRIEF_MAX_BYTES),
  ];
  for (const stage of latestStages(detail.stages, run.id)) items.push(evidence({ kind: "stage", id: stage.id }, stageStatus(stage), stageWords(stage)));
  for (const gate of detail.gates ?? []) if (gate.run === run.id) items.push(gateEvidence(gate));
  return bundleOf(`Run ${index + 1} of ${detail.item.title}`, items);
}
