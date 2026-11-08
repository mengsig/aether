// Schedules across the fleet or for one agent: list them with recurrence, policies and next run,
// pause, resume or cancel one, read its history, and create one that delivers or spawns a new agent.
// Every list lives in core's store; this file holds only drafts and toggles.

import { routeHref, useReread, useSelector } from "@aether-ui/core";
import type { ConnId } from "@aether-ui/protocol";
import { aether } from "@aether-ui/protocol";
import { Badge, Button, Field } from "@aether-ui/ui";
import { useCallback, useState } from "react";
import { invariant } from "../fleet/invariant.ts";
import { DraftError, EFFORTS } from "./commands.ts";
import { Action, isList, ReadView, RunNote, Section, useRun, type Run } from "./kit.tsx";
import { scheduleCommand, type ScheduleDraft } from "./schedule-commands.ts";

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

function SpawnedHistory({ conn, outcome }: { readonly conn: ConnId; readonly outcome: aether.ScheduleOutcomeSpawned }) {
  const agent = useSelector(useCallback((state) => state.aether[conn]?.agents[outcome.agent], [conn, outcome.agent]));
  return <>
    {" · "}{agent?.name ?? outcome.agent}{agent ? ` · ${agent.status.state}` : null}
    {" · "}<a className="ui-link" href={routeHref("#/agents/:conn/:agent", { conn, agent: outcome.agent })}>Open conversation</a>
  </>;
}

function History({ conn, id, revision }: { readonly conn: ConnId; readonly id: string; readonly revision: string }) {
  const [refresh, setRefresh] = useState(0);
  const history = useReread(conn, `schedule_history:${id}`, aether.scheduleHistory({ id, limit: 20 }), ["schedule_history"], `${revision}:${refresh}`);
  return (
    <ReadView entry={history} what="the history" empty="It has not run yet" guard={isList} isEmpty={(list) => list.length === 0} retry={() => setRefresh((n) => n + 1)}>
      {(list) => (
        <ul className="b3-list" aria-label="Schedule history">
          {(list as readonly aether.ScheduleOccurrence[]).map((entry) => (
            <li key={`${entry.due}:${entry.recorded_at}`} className="b3-meta">
              {when(entry.due)} · {entry.outcome.outcome}
              {entry.outcome.outcome === "spawned" ? <SpawnedHistory conn={conn} outcome={entry.outcome} /> : null}
            </li>
          ))}
        </ul>
      )}
    </ReadView>
  );
}

function ScheduleRow({ conn, agent, schedule, run, changed, revision }: { readonly conn: ConnId; readonly agent: string; readonly schedule: aether.Schedule; readonly run: Run; readonly changed: () => void; readonly revision: string }) {
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
      <p className="b3-meta">Schedule updated {when(schedule.updated_at)}</p>
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
      {open ? <History conn={conn} id={schedule.id} revision={revision} /> : null}
    </li>
  );
}

type Edit = (field: keyof ScheduleDraft) => (event: { target: { value: string } }) => void;

function Choice({ label, value, onChange, options }: { readonly label: string; readonly value: string; readonly onChange: (event: { target: { value: string } }) => void; readonly options: readonly (readonly [string, string])[] }) {
  return (
    <label className="b3-field">
      <span className="ui-field-label">{label}</span>
      <select className="b3-select" value={value} onChange={onChange}>
        {options.map(([option, text]) => <option key={option} value={option}>{text}</option>)}
      </select>
    </label>
  );
}

function Text({ label, value, onChange, rows }: { readonly label: string; readonly value: string; readonly onChange: (event: { target: { value: string } }) => void; readonly rows: number }) {
  return (
    <label className="b3-field">
      <span className="ui-field-label">{label}</span>
      <textarea className="b3-textarea" rows={rows} value={value} onChange={onChange} />
    </label>
  );
}

function RecurrenceFields({ draft, edit }: { readonly draft: ScheduleDraft; readonly edit: Edit }) {
  return (
    <div className="b3-form">
      <Choice label="Recurrence" value={draft.recurrence} onChange={edit("recurrence")} options={[["interval", "Every N seconds"], ["cron", "Cron"]]} />
      {draft.recurrence === "interval" ? (
        <Field label="Every (seconds)" inputMode="numeric" value={draft.seconds} onChange={edit("seconds")} />
      ) : (
        <>
          <Field label="Cron expression" placeholder="0 9 * * 1-5" value={draft.expression} onChange={edit("expression")} />
          <Field label="Timezone" placeholder="Europe/Oslo" value={draft.timezone} onChange={edit("timezone")} />
        </>
      )}
    </div>
  );
}

function SpawnFields({ draft, edit }: { readonly draft: ScheduleDraft; readonly edit: Edit }) {
  return (
    <>
      <Text label="Objective for each new agent" rows={2} value={draft.objective} onChange={edit("objective")} />
      <div className="b3-form">
        <Field label="Workspace" placeholder="/srv/repo" value={draft.workspace} onChange={edit("workspace")} />
        <Field label="Model" placeholder="anthropic/claude-opus-5-5" value={draft.model} onChange={edit("model")} />
        <Choice label="Effort" value={draft.effort} onChange={edit("effort")} options={[["", "Model default"], ...EFFORTS.map((e) => [e, e.replace("_", " ")] as const)]} />
      </div>
    </>
  );
}

function PolicyFields({ draft, edit }: { readonly draft: ScheduleDraft; readonly edit: Edit }) {
  const spawns = draft.target === "spawn";
  return (
    <div className="b3-form">
      {spawns ? null : <Choice label="When the agent is busy" value={draft.busy} onChange={edit("busy")} options={[["skip", "Skip that delivery"], ["queue_one", "Queue one"]]} />}
      <Choice label="Missed occurrences" value={draft.missed} onChange={edit("missed")} options={[["coalesce", "Catch up once"], ["skip", "Skip them"]]} />
      <Field label={spawns ? "Maximum runs (blank for unlimited)" : "Maximum deliveries (blank for unlimited)"} inputMode="numeric" value={draft.limit} onChange={edit("limit")} />
    </div>
  );
}

const newDraft = (): ScheduleDraft => ({
  name: "", target: "agent", recurrence: "interval", seconds: "3600", expression: "0 9 * * *", timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
  busy: "skip", missed: "coalesce", limit: "", message: "", objective: "", workspace: "", model: "", effort: "",
});

function NewSchedule({ conn, agent, run, changed }: { readonly conn: ConnId; readonly agent: string; readonly run: Run; readonly changed: () => void }) {
  const [draft, setDraft] = useState<ScheduleDraft>(newDraft);
  const [problem, setProblem] = useState<string | null>(null);
  const edit: Edit = (field) => (event) => setDraft((old) => ({ ...old, [field]: event.target.value }));
  const spawns = draft.target === "spawn";
  const create = async () => {
    try {
      const command = scheduleCommand(agent, draft);
      setProblem(null);
      if (await run.send(command, "Schedule created")) { setDraft(newDraft()); changed(); }
    } catch (error) {
      if (!(error instanceof DraftError)) throw error;
      setProblem(error.message);
    }
  };
  const confirm = spawns ? { title: "Start new agents on a schedule?", body: `Each occurrence starts a fresh agent in ${draft.workspace.trim() || "the workspace you named"} and spends on its own run. It asks before external side effects.`, confirmLabel: "Create schedule" } : undefined;
  return (
    <div className="b3-stack">
      <div className="b3-form">
        <Field label="Schedule name" placeholder="Morning check" value={draft.name} onChange={edit("name")} />
        <Choice label="Send to" value={draft.target} onChange={edit("target")} options={[["agent", "This agent"], ["spawn", "A new agent"]]} />
      </div>
      <RecurrenceFields draft={draft} edit={edit} />
      {spawns ? <SpawnFields draft={draft} edit={edit} /> : <Text label="Message to deliver" rows={2} value={draft.message} onChange={edit("message")} />}
      <PolicyFields draft={draft} edit={edit} />
      <div className="b3-row">
        <Action conn={conn} command={{ cmd: "create_schedule" }} label="Create schedule" busy={run.status.state === "busy"} confirm={confirm} onRun={create} />
      </div>
      {problem ? <p className="b3-note" role="alert" data-state="failed">{problem}</p> : null}
    </div>
  );
}

type Scope = "agent" | "all";

function ScheduleList({ conn, agent, scope, run, revision, changed }: { readonly conn: ConnId; readonly agent: string; readonly scope: Scope; readonly run: Run; readonly revision: string; readonly changed: () => void }) {
  const all = scope === "all";
  const command = all ? aether.listSchedules({}) : aether.listSchedules({ target: agent });
  const schedules = useReread(conn, all ? "schedules:all" : `schedules:${agent}`, command, ["schedules"], revision);
  return (
    <ReadView entry={schedules} what="the schedules" empty={all ? "No schedules on this daemon" : "No schedules deliver to this agent"} guard={isList} isEmpty={(list) => list.length === 0} retry={changed}>
      {(list) => (
        <ul className="b3-list" aria-label="Schedules">
          {(list as readonly aether.Schedule[]).map((s) => <ScheduleRow key={s.id} conn={conn} agent={agent} schedule={s} run={run} changed={changed} revision={revision} />)}
        </ul>
      )}
    </ReadView>
  );
}

export function SchedulesPanel({ conn, agent }: { readonly conn: ConnId; readonly agent: aether.Agent }) {
  invariant(agent.id !== "", "schedule controls name their agent");
  const run = useRun(conn);
  const [refresh, setRefresh] = useState(0);
  const [scope, setScope] = useState<Scope>("agent");
  const scheduleEvent = useSelector(useCallback((state) => {
    const events = state.aether[conn]?.events ?? [];
    return events.findLast((event) => event.kind.type === "custom" && event.kind.name.startsWith("schedule.") && event.kind.name !== "schedule.occurrence")?.seq ?? 0;
  }, [conn]));
  const revision = `${scheduleEvent}:${refresh}`;
  const changed = () => setRefresh((n) => n + 1);
  return (
    <>
      <Section title="Schedules" hint="Recurring work: messages to an agent, or new agents started on a timer.">
        <div className="b3-row">
          <Button variant={scope === "agent" ? "primary" : "quiet"} size="sm" aria-pressed={scope === "agent"} onClick={() => setScope("agent")}>This agent's schedules</Button>
          <Button variant={scope === "all" ? "primary" : "quiet"} size="sm" aria-pressed={scope === "all"} onClick={() => setScope("all")}>All schedules</Button>
          <Button variant="quiet" size="sm" onClick={changed}>Refresh schedules</Button>
        </div>
        <ScheduleList conn={conn} agent={agent.id} scope={scope} run={run} revision={revision} changed={changed} />
      </Section>
      <Section title="New schedule" hint="Choose a recurrence, what it sends and how missed or overlapping occurrences are handled.">
        <NewSchedule conn={conn} agent={agent.id} run={run} changed={changed} />
      </Section>
      <RunNote status={run.status} />
    </>
  );
}
