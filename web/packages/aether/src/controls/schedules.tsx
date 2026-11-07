// Schedules across the fleet or for one agent: list them with recurrence, policies and next run,
// pause, resume or cancel one, read its history, and create one that delivers or spawns a new agent.
// Every list lives in core's store; this file holds only drafts and toggles.

import { useDraft, useReread } from "@aether-ui/core";
import type { ConnId } from "@aether-ui/protocol";
import { aether } from "@aether-ui/protocol";
import { Badge, Button, Field, Segmented, TextArea } from "@aether-ui/ui";
import { useState } from "react";
import { invariant } from "../fleet/invariant.ts";
import { DraftError } from "./commands.ts";
import { Action, isList, ReadView, RunNote, Section, useRun, type Run } from "./kit.tsx";
import { RecurrenceFields, SpawnFields, PolicyFields, type ScheduleForm } from "./schedule-fields.tsx";
import { scheduleCommand } from "./schedule-commands.ts";

function when(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}

function statusText(status: aether.ScheduleStatus): string {
  const record = status as { state: string; next_due?: string; error?: string };
  if (record.state === "active") return `Next ${when(record.next_due ?? "")}`;
  if (record.state === "paused") return "Paused";
  if (record.state === "failed") return `Failed: ${record.error ?? "no reason given"}`;
  return record.state === "cancelled" ? "Cancelled" : "Finished";
}

function interval(seconds: number): string {
  if (seconds % 3600 === 0) return `Every ${seconds / 3600} h`;
  return seconds % 60 === 0 ? `Every ${seconds / 60} min` : `Every ${seconds} s`;
}

function every(recurrence: aether.Recurrence): string {
  return recurrence.kind === "interval" ? interval(recurrence.seconds) : `Cron ${recurrence.expression} · ${recurrence.timezone}`;
}

const BUSY_TEXT: Record<aether.BusyPolicy, string> = { queue_one: "if busy, queue one", skip: "if busy, skip" };
const MISSED_TEXT: Record<aether.MissedRunPolicy, string> = { coalesce: "missed runs catch up once", skip: "missed runs are skipped" };

function policyText(schedule: aether.Schedule): string {
  const spawns = schedule.spawn != null;
  const limit = schedule.max_deliveries == null ? "unlimited" : `up to ${schedule.max_deliveries} ${spawns ? "runs" : "deliveries"}`;
  const parts = [spawns ? null : BUSY_TEXT[schedule.busy ?? "skip"], MISSED_TEXT[schedule.missed], limit];
  return parts.filter((part) => part !== null).join(" · ");
}

function targetText(schedule: aether.Schedule, agent: string): string {
  if (schedule.spawn != null) return `Starts a new agent: ${schedule.spawn.objective}`;
  return schedule.target === agent ? "Delivers to this agent" : `Delivers to ${schedule.target ?? "an agent"}`;
}

function History({ conn, id }: { readonly conn: ConnId; readonly id: string }) {
  const [refresh, setRefresh] = useState(0);
  const history = useReread(conn, `schedule_history:${id}`, aether.scheduleHistory({ id, limit: 20 }), ["schedule_history"], refresh);
  return (
    <ReadView entry={history} what="the history" empty="It has not run yet" guard={isList} isEmpty={(list) => list.length === 0} retry={() => setRefresh((n) => n + 1)}>
      {(list) => (
        <ul className="b3-list" aria-label="Schedule history">
          {(list as readonly aether.ScheduleOccurrence[]).map((entry) => (
            <li key={entry.recorded_at} className="b3-meta">{when(entry.due)} · {(entry.outcome as { outcome?: string }).outcome ?? "recorded"}</li>
          ))}
        </ul>
      )}
    </ReadView>
  );
}

function ScheduleRow({ conn, agent, schedule, run, changed }: { readonly conn: ConnId; readonly agent: string; readonly schedule: aether.Schedule; readonly run: Run; readonly changed: () => void }) {
  const [open, setOpen] = useState(false);
  const state = (schedule.status as { state: string }).state;
  const live = state === "active" || state === "paused";
  const toggle = aether.updateSchedule({ id: schedule.id, action: state === "paused" ? "resume" : "pause" });
  const stop = aether.updateSchedule({ id: schedule.id, action: "cancel" });
  const busy = run.status.state === "busy";
  return (
    <li className="b3-item">
      <div className="b3-item-head">
        <p className="b3-item-title">{schedule.name}</p>
        <Badge tone={state === "active" ? "success" : state === "failed" ? "danger" : "neutral"}>{statusText(schedule.status)}</Badge>
      </div>
      <p className="b3-meta">{every(schedule.recurrence)} · {schedule.deliveries} {schedule.spawn != null ? "started" : "delivered"}</p>
      <p className="b3-meta">{targetText(schedule, agent)}</p>
      <p className="b3-meta">{policyText(schedule)}</p>
      <div className="b3-row">
        {live ? <Action conn={conn} command={toggle} label={state === "paused" ? "Resume" : "Pause"} busy={busy} onRun={async () => { if (await run.send(toggle, "Schedule updated")) changed(); }} /> : null}
        {live ? (
          <Action conn={conn} command={stop} label="Cancel schedule" variant="danger" busy={busy}
            confirm={{ title: `Cancel ${schedule.name}?`, body: "It stops for good; a cancelled schedule cannot be resumed.", confirmLabel: "Cancel schedule" }}
            onRun={async () => { if (await run.send(stop, "Schedule cancelled")) changed(); }} />
        ) : null}
        <Button variant="quiet" size="sm" aria-expanded={open} onClick={() => setOpen((o) => !o)}>{open ? "Hide history" : "History"}</Button>
      </div>
      {open ? <History conn={conn} id={schedule.id} /> : null}
    </li>
  );
}

const newDraft = (): ScheduleForm => ({
  customInterval: false, customCron: false, name: "", target: "agent", recurrence: "interval", seconds: "3600", expression: "0 9 * * *", timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
  busy: "skip", missed: "coalesce", limit: "", message: "", objective: "", workspace: "", model: "", effort: "",
});

function NewSchedule({ conn, agent, run, changed }: { readonly conn: ConnId; readonly agent: string; readonly run: Run; readonly changed: () => void }) {
  const form = useDraft(`controls/${agent}/schedule`, { ...newDraft() });
  const draft = form.value;
  const [problem, setProblem] = useState<string | null>(null);
  const edit = form.update;
  const spawns = draft.target === "spawn";
  const create = async () => {
    try {
      const command = scheduleCommand(agent, draft);
      setProblem(null);
      if (await run.send(command, "Schedule created")) { form.clearIfUnchanged(); changed(); }
    } catch (error) {
      if (!(error instanceof DraftError)) throw error;
      setProblem(error.message);
    }
  };
  const confirm = spawns ? { title: "Start new agents on a schedule?", body: `Each occurrence starts a fresh agent in ${draft.workspace.trim() || "the workspace you named"} and spends on its own run. It asks before external side effects.`, confirmLabel: "Create schedule" } : undefined;
  return (
    <div className="b3-stack">
      <div className="b3-form">
        <Field label="Schedule name" data-testid="schedule-name" placeholder="Morning check" value={draft.name} onChange={e => edit({ name: e.target.value })} />
        <Segmented label="Send to" value={draft.target} onChange={target => edit({ target: target as ScheduleForm["target"] })} options={[{ id: "agent", label: "This agent" }, { id: "spawn", label: "A new agent" }]} />
      </div>
      <RecurrenceFields draft={draft} edit={edit} />
      {spawns ? <SpawnFields conn={conn} draft={draft} edit={edit} /> : <TextArea label="Message to deliver" rows={2} value={draft.message} onChange={e => edit({ message: e.target.value })} />}
      <PolicyFields draft={draft} edit={edit} />
      <div className="b3-row">
        <Action conn={conn} command={{ cmd: "create_schedule" }} label="Create schedule" variant="primary" data-testid="schedule-submit" blocked={scheduleProblem(agent, draft)} busy={run.status.state === "busy"} confirm={confirm} onRun={create} />
      </div>
      {problem ? <p className="b3-note" role="alert" data-state="failed">{problem}</p> : null}
    </div>
  );
}

type Scope = "agent" | "all";

function ScheduleList({ conn, agent, scope, run, refresh, changed }: { readonly conn: ConnId; readonly agent: string; readonly scope: Scope; readonly run: Run; readonly refresh: number; readonly changed: () => void }) {
  const all = scope === "all";
  const command = all ? aether.listSchedules({}) : aether.listSchedules({ target: agent });
  const schedules = useReread(conn, all ? "schedules:all" : `schedules:${agent}`, command, ["schedules"], refresh);
  return (
    <ReadView entry={schedules} what="the schedules" empty={all ? "No schedules on this daemon" : "No schedules deliver to this agent"} guard={isList} isEmpty={(list) => list.length === 0} retry={changed}>
      {(list) => (
        <ul className="b3-list" aria-label="Schedules">
          {(list as readonly aether.Schedule[]).map((s) => <ScheduleRow key={s.id} conn={conn} agent={agent} schedule={s} run={run} changed={changed} />)}
        </ul>
      )}
    </ReadView>
  );
}

export function SchedulesPanel({ conn, agent }: { readonly conn: ConnId; readonly agent: aether.Agent }) {
  invariant(agent.id !== "", "schedule controls name their agent");
  const run = useRun(conn);
  const [refresh, setRefresh] = useState(0);
  const filter = useDraft(`controls/${agent.id}/schedule-filter`, { scope: "agent" as Scope });
  const { scope } = filter.value;
  const changed = () => setRefresh((n) => n + 1);
  return (
    <>
      <Section title="Schedules" hint="Recurring work: messages to an agent, or new agents started on a timer.">
        <Segmented label="Schedule scope" value={scope} onChange={scope => filter.update({ scope: scope as Scope })}
          options={[{ id: "agent", label: "This agent's schedules" }, { id: "all", label: "All schedules" }]} />
        <ScheduleList conn={conn} agent={agent.id} scope={scope} run={run} refresh={refresh} changed={changed} />
      </Section>
      <Section title="New schedule" hint="Choose a recurrence, what it sends and how missed or overlapping occurrences are handled.">
        <NewSchedule conn={conn} agent={agent.id} run={run} changed={changed} />
      </Section>
      <RunNote status={run.status} />
    </>
  );
}

function scheduleProblem(agent: string, draft: ScheduleForm): string | null {
  try { scheduleCommand(agent, draft); return null; }
  catch (error) { if (!(error instanceof DraftError)) throw error; return error.message; }
}
