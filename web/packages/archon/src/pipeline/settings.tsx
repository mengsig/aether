import type { archon } from "@aether-ui/protocol";
import { modelLabel } from "./StageAttempt.tsx";

type RecordValue = Record<string, unknown>;

function record(value: unknown): RecordValue | null {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value as RecordValue : null;
}

export function settingsFor(plan: archon.RunPlan | null, stage: string): readonly RecordValue[] | null {
  const settings = record(plan)?.settings;
  if (!Array.isArray(settings)) return null;
  return settings.map(record).filter((entry): entry is RecordValue => entry !== null && (entry.key === stage || entry.key === `${stage}.review`));
}

export function deadlineFor(settings: readonly RecordValue[] | null, stage: archon.StageRun): string | null {
  if (stage.state.state !== "running") return null;
  const limit = settings?.find((entry) => entry.key === stage.stage_id)?.timeout_minutes;
  const start = Date.parse(stage.started_at);
  if (typeof limit !== "number" || !Number.isFinite(limit) || limit < 0 || !Number.isFinite(start)) return null;
  const end = new Date(start + limit * 60_000);
  return Number.isFinite(end.getTime()) ? end.toISOString() : null;
}

function source(value: unknown): string {
  return value === "default" || value === "override" || value === "tier" || value === "exact" ? value : "not recorded";
}

function choiceLabel(value: unknown): string {
  const choice = record(value);
  const model = choice?.model;
  const ref = typeof model === "string" ? model : record(model);
  const effort = typeof choice?.effort === "string" ? choice.effort : "not recorded";
  return `${ref === null ? "Model not recorded" : modelLabel(ref)} · effort ${effort}`;
}

function SettingsEntry({ entry }: { readonly entry: RecordValue }) {
  const choices = Array.isArray(entry.choices) ? entry.choices : [];
  const timeout = typeof entry.timeout_minutes === "number" && Number.isFinite(entry.timeout_minutes) && entry.timeout_minutes >= 0
    ? `${entry.timeout_minutes} min (${source(entry.timeout)})` : "not recorded";
  return <div>
    {typeof entry.key === "string" && entry.key.endsWith(".review") ? <strong>Delegated reviewers: </strong> : null}
    {choices.length ? choices.map((choice, index) => <div key={index}>{choiceLabel(choice)} · model {source(entry.model)} · effort {source(entry.effort)}</div>) : <div>Model and effort not recorded</div>}
    <div>Limit: {timeout}</div>
  </div>;
}

export function StageSettings({ settings }: { readonly settings: readonly RecordValue[] | null }) {
  if (settings === null) return <p className="pipeline-terms">Effective settings unavailable on this Archon version.</p>;
  if (settings.length === 0) return <p className="pipeline-terms">Effective settings not recorded.</p>;
  return <div className="pipeline-terms" aria-label="Resolved plan settings">{settings.map((entry, index) => <SettingsEntry key={index} entry={entry} />)}</div>;
}
