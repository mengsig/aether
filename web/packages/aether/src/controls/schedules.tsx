// Schedules that deliver to one agent: list them with their next run, pause, resume or cancel one,
// read its history, and create a new interval schedule. Every list lives in core's store.

import { useReread } from "@aether-ui/core";
import type { ConnId } from "@aether-ui/protocol";
import { aether } from "@aether-ui/protocol";
import { Badge, Button, Field } from "@aether-ui/ui";
import { useState } from "react";
import { invariant } from "../fleet/invariant.ts";
import { DraftError, scheduleCommand, type ScheduleDraft } from "./commands.ts";
import { Action, isList, ReadView, RunNote, Section, useRun, type Run } from "./kit.tsx";

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

function every(recurrence: aether.Recurrence): string {
  const record = recurrence as { kind: string; seconds?: number; expression?: string };
  if (record.kind === "interval") return `Every ${Math.round((record.seconds ?? 0) / 60)} min`;
  return `Cron ${record.expression ?? ""}`;
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

function ScheduleRow({ conn, schedule, run, changed }: { readonly conn: ConnId; readonly schedule: aether.Schedule; readonly run: Run; readonly changed: () => void }) {
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
      <p className="b3-meta">{every(schedule.recurrence)} · {schedule.deliveries} delivered</p>
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

const EMPTY_DRAFT: ScheduleDraft = { name: "", seconds: "3600", message: "" };

function NewSchedule({ conn, agent, run, changed }: { readonly conn: ConnId; readonly agent: string; readonly run: Run; readonly changed: () => void }) {
  const [draft, setDraft] = useState<ScheduleDraft>(EMPTY_DRAFT);
  const [problem, setProblem] = useState<string | null>(null);
  const edit = (field: keyof ScheduleDraft) => (event: { target: { value: string } }) => setDraft((old) => ({ ...old, [field]: event.target.value }));
  const create = async () => {
    try {
      const command = scheduleCommand(agent, draft);
      setProblem(null);
      if (await run.send(command, "Schedule created")) { setDraft(EMPTY_DRAFT); changed(); }
    } catch (error) {
      if (!(error instanceof DraftError)) throw error;
      setProblem(error.message);
    }
  };
  return (
    <div className="b3-stack">
      <div className="b3-form">
        <Field label="Schedule name" placeholder="Morning check" value={draft.name} onChange={edit("name")} />
        <Field label="Every (seconds)" inputMode="numeric" value={draft.seconds} onChange={edit("seconds")} />
      </div>
      <label className="b3-field">
        <span className="ui-field-label">Message to deliver</span>
        <textarea className="b3-textarea" rows={2} value={draft.message} onChange={edit("message")} />
      </label>
      <div className="b3-row">
        <Action conn={conn} command={{ cmd: "create_schedule" }} label="Create schedule" busy={run.status.state === "busy"} onRun={create} />
      </div>
      {problem ? <p className="b3-note" role="alert" data-state="failed">{problem}</p> : null}
    </div>
  );
}

export function SchedulesPanel({ conn, agent }: { readonly conn: ConnId; readonly agent: aether.Agent }) {
  invariant(agent.id !== "", "schedule controls name their agent");
  const run = useRun(conn);
  const [refresh, setRefresh] = useState(0);
  const changed = () => setRefresh((n) => n + 1);
  const schedules = useReread(conn, `schedules:${agent.id}`, aether.listSchedules({ target: agent.id }), ["schedules"], refresh);
  return (
    <>
      <Section title="Schedules" hint="Recurring messages delivered to this agent.">
        <ReadView entry={schedules} what="the schedules" empty="No schedules deliver to this agent" guard={isList} isEmpty={(list) => list.length === 0} retry={changed}>
          {(list) => (
            <ul className="b3-list" aria-label="Schedules">
              {(list as readonly aether.Schedule[]).map((s) => <ScheduleRow key={s.id} conn={conn} schedule={s} run={run} changed={changed} />)}
            </ul>
          )}
        </ReadView>
      </Section>
      <Section title="New schedule" hint="Delivers the message on an interval; a busy agent skips that delivery.">
        <NewSchedule conn={conn} agent={agent.id} run={run} changed={changed} />
      </Section>
      <RunNote status={run.status} />
    </>
  );
}
