// A run's pipeline as steps: the stages `run_plan` says the run will take, each with the stage runs it has
// had. Without `run_plan` (protocol 23) the steps are the stages the run has started, in the order it did.

import type { archon } from "@aether-ui/protocol";
import type { Status } from "@aether-ui/ui";
import { stagePill } from "../item/facts.ts";
import { invariant } from "../queue/model.ts";

export interface PipelineStep {
  readonly id: string;
  /** What the plan says of the stage; null for a stage only seen in the run's history. */
  readonly planned: archon.StageSummary | null;
  readonly attempts: readonly archon.StageRun[];
  readonly status: Status;
  readonly label: string;
  readonly current: boolean;
}

export interface PipelineView {
  readonly source: "plan" | "observed";
  readonly pipeline: string;
  readonly revision: string | null;
  readonly edited: boolean;
  readonly steps: readonly PipelineStep[];
}

const RANK: Readonly<Record<archon.StageStateKind, number>> = { running: 0, spawning: 1, failed: 2, timed_out: 3, cancelled: 4, completed: 5 };

/** The step's state is its latest attempt's; among fan-out slots, the one still running or failing wins. */
function latest(attempts: readonly archon.StageRun[]): archon.StageRun | null {
  if (attempts.length === 0) return null;
  const top = Math.max(...attempts.map((stage) => stage.attempt));
  const last = attempts.filter((stage) => stage.attempt === top).sort((a, b) => RANK[a.state.state] - RANK[b.state.state]);
  invariant(last.length > 0, "the latest attempt has a stage run");
  return last[0] ?? null;
}

function currentStage(run: archon.PipelineRun): string | null {
  const state = run.state;
  return state.state === "running" || state.state === "awaiting_operator" ? state.stage_id : null;
}

function step(id: string, planned: archon.StageSummary | null, run: archon.PipelineRun, attempts: readonly archon.StageRun[]): PipelineStep {
  invariant(attempts.every((stage) => stage.stage_id === id && stage.run === run.id), `the attempts of ${id} belong to ${run.id}`);
  const current = currentStage(run) === id;
  if (current && run.state.state === "awaiting_operator") return { id, planned, attempts, status: "operator", label: "Needs you", current };
  const shown = latest(attempts);
  if (shown) return { id, planned, attempts, ...stagePill(shown.state), current };
  const open = run.state.state === "running" || run.state.state === "awaiting_operator";
  return { id, planned, attempts, status: open ? "queued" : "cancelled", label: open ? "Not started" : "Not run", current };
}

/** Stage ids in the order the run first started them, with the current stage last if it has no run yet. */
export function observedOrder(run: archon.PipelineRun, stages: readonly archon.StageRun[]): string[] {
  const order: string[] = [];
  for (const stage of stages) if (stage.run === run.id && !order.includes(stage.stage_id)) order.push(stage.stage_id);
  const current = currentStage(run);
  if (current !== null && !order.includes(current)) order.push(current);
  return order;
}

export function pipelineView(run: archon.PipelineRun, stages: readonly archon.StageRun[], plan: archon.RunPlan | null): PipelineView {
  invariant(plan === null || plan.run === run.id, `the plan of ${plan?.run ?? ""} is not the plan of ${run.id}`);
  const mine = stages.filter((stage) => stage.run === run.id);
  const attemptsOf = (id: string) => mine.filter((stage) => stage.stage_id === id);
  if (plan === null) {
    const steps = observedOrder(run, mine).map((id) => step(id, null, run, attemptsOf(id)));
    return { source: "observed", pipeline: run.pipeline, revision: null, edited: false, steps };
  }
  invariant(new Set(plan.stages.map((stage) => stage.id)).size === plan.stages.length, "a plan names each stage once");
  const planned = plan.stages.map((stage) => step(stage.id, stage, run, attemptsOf(stage.id)));
  // A stage an edit removed after it ran still happened; it is listed after the plan.
  const known = new Set(plan.stages.map((stage) => stage.id));
  const extra = observedOrder(run, mine).filter((id) => !known.has(id)).map((id) => step(id, null, run, attemptsOf(id)));
  return { source: "plan", pipeline: plan.pipeline, revision: plan.revision, edited: plan.edited, steps: [...planned, ...extra] };
}

/** The plan's words for a stage: kind, gate, fan-out, loop and limits. */
export function stageTerms(stage: archon.StageSummary): string[] {
  invariant(stage.fan_out >= 1, `stage ${stage.id} runs at least one slot`);
  return [
    stage.kind,
    stage.gate !== "none" ? `gate ${stage.gate.replace("_", " ")}` : null,
    stage.fan_out > 1 ? `${stage.fan_out} in parallel` : null,
    stage.loops_to ? `loops to ${stage.loops_to} (max ${stage.max_loops})` : null,
  ].filter((term): term is string => term !== null);
}
