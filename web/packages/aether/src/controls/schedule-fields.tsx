import { useReread } from "@aether-ui/core";
import { Combo, Field, Picker, Segmented, TextArea } from "@aether-ui/ui";
import { Workspace } from "../activity/Workspace.tsx";
import { Models } from "../activity/Models.tsx";
import { EFFORTS } from "./commands.ts";
import type { ScheduleDraft } from "./schedule-commands.ts";
import { CRON_PRESETS, INTERVAL_PRESETS, describeCron, timezoneOptions } from "./schedule-presets.ts";

export type ScheduleForm = ScheduleDraft & { customInterval: boolean; customCron: boolean };
export type Edit = (patch: Partial<ScheduleForm>) => void;
const custom = { id: "custom", label: "Custom" };
const zones = timezoneOptions();

export function RecurrenceFields({ draft: d, edit }: { draft: ScheduleForm; edit: Edit }) {
  return <div className="b3-form">
    <Segmented label="Recurrence" value={d.recurrence} onChange={recurrence => edit({ recurrence: recurrence as ScheduleDraft["recurrence"] })}
      options={[{ id: "interval", label: "Every N seconds" }, { id: "cron", label: "Cron" }]} />
    {d.recurrence === "interval" ? <>
      <Picker label="Every (seconds)" value={d.customInterval ? "custom" : d.seconds} options={[...INTERVAL_PRESETS, custom]}
        onChange={seconds => edit(seconds === "custom" ? { customInterval: true } : { seconds, customInterval: false })} />
      {d.customInterval ? <Field label="Custom interval (seconds)" inputMode="numeric" value={d.seconds} onChange={e => edit({ seconds: e.target.value })} /> : null}
    </> : <>
      <Picker label="Cron preset" data-testid="schedule-cron-preset" value={d.customCron ? "custom" : d.expression} options={[...CRON_PRESETS, custom]}
        onChange={expression => edit(expression === "custom" ? { customCron: true } : { expression, customCron: false })} />
      {d.customCron ? <Field label="Cron expression" value={d.expression} onChange={e => edit({ expression: e.target.value })} /> : null}
      <p role="status">{describeCron(d.expression)}</p>
      <Combo label="Timezone" data-testid="schedule-timezone" value={d.timezone} onChange={timezone => edit({ timezone })}
        suggestions={zones.filter(z => z.id.toLowerCase().includes(d.timezone.toLowerCase()))} />
    </>}
  </div>;
}

export function SpawnFields({ conn, draft: d, edit }: { conn: string; draft: ScheduleForm; edit: Edit }) {
  const recent = useReread(conn, "start:workspaces", { cmd: "list_workspaces" }, ["workspaces"]);
  return <>
    <TextArea label="Objective for each new agent" rows={2} value={d.objective} onChange={e => edit({ objective: e.target.value })} />
    <Workspace conn={conn} path={d.workspace} change={workspace => edit({ workspace })} recent={recent} testId="schedule-workspace" />
    <Models conn={conn} screen="schedule" value={d.model} onChange={model => edit({ model })} testId="schedule-model" />
    <Segmented label="Effort" value={d.effort} onChange={effort => edit({ effort })}
      options={[{ id: "", label: "Model default" }, ...EFFORTS.map(id => ({ id, label: id.replace("_", " ") }))]} />
  </>;
}

export function PolicyFields({ draft: d, edit }: { draft: ScheduleForm; edit: Edit }) {
  const spawns = d.target === "spawn";
  return <div className="b3-form">
    {!spawns ? <Picker label="When the agent is busy" value={d.busy} onChange={busy => edit({ busy })}
      options={[{ id: "skip", label: "Skip that delivery" }, { id: "queue_one", label: "Queue one" }]} /> : null}
    <Picker label="Missed occurrences" value={d.missed} onChange={missed => edit({ missed })}
      options={[{ id: "coalesce", label: "Catch up once" }, { id: "skip", label: "Skip them" }]} />
    <Field label={spawns ? "Maximum runs (blank for unlimited)" : "Maximum deliveries (blank for unlimited)"} inputMode="numeric" value={d.limit} onChange={e => edit({ limit: e.target.value })} />
  </div>;
}
