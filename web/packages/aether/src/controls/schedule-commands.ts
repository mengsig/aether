// Pure builders for the schedule form: a draft becomes one create_schedule command, or throws a
// DraftError whose message a person can act on. A schedule delivers to this agent or spawns a new one.

import { aether } from "@aether-ui/protocol";
import { invariant } from "../fleet/invariant.ts";
import { DraftError, EFFORTS, modelRef } from "./commands.ts";

export const MIN_INTERVAL_SECONDS = 60;
export const BUSY_POLICIES: readonly aether.BusyPolicy[] = ["queue_one", "skip"];
export const MISSED_POLICIES: readonly aether.MissedRunPolicy[] = ["coalesce", "skip"];

export type RecurrenceKind = "interval" | "cron";
export type ScheduleTarget = "agent" | "spawn";

export interface ScheduleDraft {
  readonly name: string;
  readonly target: ScheduleTarget;
  readonly recurrence: RecurrenceKind;
  readonly seconds: string;
  readonly expression: string;
  readonly timezone: string;
  readonly busy: string;
  readonly missed: string;
  /** Blank means unlimited. */
  readonly limit: string;
  /** What a delivery schedule sends the agent. */
  readonly message: string;
  /** What each spawned agent is asked to do. */
  readonly objective: string;
  readonly workspace: string;
  readonly model: string;
  /** Blank keeps the model's default effort. */
  readonly effort: string;
}

function need(condition: boolean, message: string): asserts condition {
  if (!condition) throw new DraftError(message);
}

function wholeNumber(text: string, label: string, minimum: number): number {
  const value = text.trim();
  need(/^\d+$/.test(value), `${label} must be a whole number`);
  const number = Number(value);
  need(Number.isSafeInteger(number) && number >= minimum, `${label} must be at least ${minimum}`);
  return number;
}

function knownTimezone(timezone: string): void {
  try {
    new Intl.DateTimeFormat("en", { timeZone: timezone });
  } catch (error) {
    throw new DraftError(`"${timezone}" is not an IANA timezone such as Europe/Oslo`, { cause: error });
  }
}

export function recurrenceOf(draft: ScheduleDraft): aether.Recurrence {
  invariant(draft.recurrence === "interval" || draft.recurrence === "cron", "a schedule recurs on an interval or a cron expression");
  if (draft.recurrence === "interval") {
    const seconds = wholeNumber(draft.seconds, "The interval", 1);
    need(seconds >= MIN_INTERVAL_SECONDS, `The interval is at least ${MIN_INTERVAL_SECONDS} seconds`);
    return { kind: "interval", seconds };
  }
  const expression = draft.expression.trim().split(/\s+/).join(" ");
  need(expression.split(" ").length === 5, "A cron expression has five fields: minute, hour, day of month, month, day of week");
  const timezone = draft.timezone.trim();
  need(timezone !== "", "Name the timezone the cron expression runs in");
  knownTimezone(timezone);
  return { kind: "cron", expression, timezone };
}

function policies(draft: ScheduleDraft): { busy: aether.BusyPolicy; missed: aether.MissedRunPolicy } {
  const busy = BUSY_POLICIES.find((policy) => policy === draft.busy);
  const missed = MISSED_POLICIES.find((policy) => policy === draft.missed);
  need(busy !== undefined, "Choose what happens when the agent is busy");
  need(missed !== undefined, "Choose what happens to missed occurrences");
  return { busy, missed };
}

function spawnOf(draft: ScheduleDraft): aether.SpawnSpec {
  const objective = draft.objective.trim();
  need(objective !== "", "Write the objective each new agent receives");
  const workspace = draft.workspace.trim();
  need(workspace.startsWith("/"), "Use an absolute workspace path on the host");
  const effort = EFFORTS.find((candidate) => candidate === draft.effort);
  need(draft.effort === "" || effort !== undefined, "Choose a valid effort");
  return {
    role: "worker",
    name: draft.name.trim(),
    objective,
    model: modelRef(draft.model),
    scope: { workspace_root: workspace },
    approval: { require_for: ["external"] },
    ...(effort === undefined ? {} : { config: { effort } }),
  };
}

/** One create_schedule command for the draft: delivered to `agent`, or spawning fresh agents. */
export function scheduleCommand(agent: string, draft: ScheduleDraft): aether.CreateScheduleCommand {
  invariant(agent !== "", "a schedule names its agent");
  invariant(draft.target === "agent" || draft.target === "spawn", "a schedule delivers to the agent or spawns a new one");
  const name = draft.name.trim();
  need(name !== "", "Name the schedule");
  const recurrence = recurrenceOf(draft);
  const { busy, missed } = policies(draft);
  const limit = draft.limit.trim() === "" ? null : wholeNumber(draft.limit, "The maximum deliveries", 1);
  if (draft.target === "spawn") {
    return aether.createSchedule({ spec: { name, recurrence, missed, max_deliveries: limit, spawn: spawnOf(draft) } });
  }
  const payload = draft.message.trim();
  need(payload !== "", "Write the message the schedule delivers");
  return aether.createSchedule({ spec: { name, target: agent, recurrence, payload, busy, missed, max_deliveries: limit } });
}
