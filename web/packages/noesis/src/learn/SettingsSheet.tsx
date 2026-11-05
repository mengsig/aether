// Noesis settings with today's spend against its budgets (CONTRACTS.md §6.1). The switch, budgets and
// billing are the host's; other devices see them read-only. Saving sends only what changed.

import type { aether } from "@aether-ui/protocol";
import { Button, Field, Sheet } from "@aether-ui/ui";
import { useMemo, useState, type FormEvent } from "react";
import { useGate, useHostAdmin, useNoesisConfigure, useNoesisRead, useOperateRefusal } from "./data.ts";
import { budgetShare, invariant, spentUsd, usd } from "./model.ts";

interface Draft {
  readonly enabled: boolean;
  readonly allowApi: boolean;
  readonly interactiveUsd: string;
  readonly backgroundUsd: string;
  readonly backgroundTokens: string;
  readonly strong: string;
  readonly cheap: string;
  readonly extraction: "manual" | "auto";
}

function draftOf(settings: aether.NoesisSettingsView): Draft {
  return {
    enabled: settings.enabled, allowApi: settings.billing.allow_api,
    interactiveUsd: String(settings.budget.interactive_usd_daily), backgroundUsd: String(settings.budget.background_usd_daily),
    backgroundTokens: String(settings.budget.background_tokens_daily),
    strong: settings.models.strong ?? "", cheap: settings.models.cheap ?? "", extraction: settings.extraction.sessions,
  };
}

/** A dollar amount ≥ 0, or why not. */
function amount(text: string): number | string {
  const value = Number(text);
  return text.trim() !== "" && Number.isFinite(value) && value >= 0 ? value : "Enter an amount of 0 or more";
}

function tokens(text: string): number | string {
  const value = Number(text);
  return text.trim() !== "" && Number.isSafeInteger(value) && value >= 0 ? value : "Enter a whole number of 0 or more";
}

/** The patch that turns `settings` into `draft`, or the field errors that stop it. */
export function patchOf(settings: aether.NoesisSettingsView, draft: Draft): { patch: aether.NoesisSettingsPatch; errors: Record<string, string> } {
  const errors: Record<string, string> = {};
  const patch: Record<string, unknown> = {};
  if (draft.enabled !== settings.enabled) patch.enabled = draft.enabled;
  if (draft.allowApi !== settings.billing.allow_api) patch.billing = { allow_api: draft.allowApi };
  const budget: Record<string, number> = {};
  const fields = [["interactive_usd_daily", amount(draft.interactiveUsd)], ["background_usd_daily", amount(draft.backgroundUsd)],
    ["background_tokens_daily", tokens(draft.backgroundTokens)]] as const;
  for (const [key, value] of fields) {
    if (typeof value === "string") errors[key] = value;
    else if (value !== settings.budget[key]) budget[key] = value;
  }
  if (Object.keys(budget).length > 0) patch.budget = budget;
  const models: Record<string, string | null> = {};
  if (draft.strong.trim() !== (settings.models.strong ?? "")) models.strong = draft.strong.trim() || null;
  if (draft.cheap.trim() !== (settings.models.cheap ?? "")) models.cheap = draft.cheap.trim() || null;
  if (Object.keys(models).length > 0) patch.models = models;
  if (draft.extraction !== settings.extraction.sessions) patch.extraction = { sessions: draft.extraction };
  invariant(Object.keys(errors).length === 0 || Object.keys(errors).every((key) => key in settings.budget), "only budget fields are checked here");
  return { patch: patch as aether.NoesisSettingsPatch, errors };
}

export function SettingsSheet({ open, conn, onClose }: { open: boolean; conn: string; onClose: () => void }) {
  const gate = useGate();
  // Spend moves all day: opening the sheet rereads the status, and nothing is read while it is closed.
  const read = useNoesisRead(conn, open ? { kind: "status" } : null);
  const status = gate.state === "on" || gate.state === "off" ? gate.status : null;
  return (
    <Sheet open={open} onClose={onClose} title="Noesis settings" description="Learning on this daemon, and what it may spend each day.">
      {read.error !== null && <p className="learn-callout" role="alert">Could not refresh the settings: {read.error}</p>}
      {status === null ? <p className="learn-muted">Loading settings…</p> : <SettingsForm key={JSON.stringify(status.settings)} conn={conn} status={status} onDone={onClose} />}
    </Sheet>
  );
}

function SettingsForm({ conn, status, onDone }: { conn: string; status: aether.NoesisStatus; onDone: () => void }) {
  const host = useHostAdmin(conn);
  const refusal = useOperateRefusal(conn);
  const configure = useNoesisConfigure(conn);
  const [draft, setDraft] = useState<Draft>(() => draftOf(status.settings));
  const [saving, setSaving] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const { patch, errors } = useMemo(() => patchOf(status.settings, draft), [status.settings, draft]);
  const dirty = Object.keys(patch).length > 0;
  const set = <K extends keyof Draft>(key: K, value: Draft[K]) => setDraft((old) => ({ ...old, [key]: value }));
  const hostNote = host ? undefined : "Only the daemon's own host can change this";
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!dirty || Object.keys(errors).length > 0) return;
    setSaving(true);
    const result = await configure(patch);
    setSaving(false);
    if (!result.ok) return setFailure(result.error);
    setFailure(null);
    onDone();
  };
  return (
    <form className="learn-settings" onSubmit={(event) => void submit(event)}>
      <Spend status={status} />
      <fieldset className="learn-fieldset">
        <legend>Noesis</legend>
        <Toggle label="Turn on Noesis" hint={hostNote ?? "Teaching, practice and the knowledge graph on this daemon"} checked={draft.enabled} disabled={!host} onChange={(v) => set("enabled", v)} />
        <Toggle label="Allow API-billed models" hint={hostNote ?? "Charged per request. Off means subscription models only."} checked={draft.allowApi} disabled={!host} onChange={(v) => set("allowApi", v)} />
      </fieldset>
      <fieldset className="learn-fieldset">
        <legend>Daily budgets</legend>
        <Field label="Teaching, in dollars" inputMode="decimal" value={draft.interactiveUsd} disabled={!host} hint={hostNote ?? "0 stops teaching for the day"} {...errorOf(errors.interactive_usd_daily)} onChange={(e) => set("interactiveUsd", e.target.value)} />
        <Field label="Background work, in dollars" inputMode="decimal" value={draft.backgroundUsd} disabled={!host} {...errorOf(errors.background_usd_daily)} onChange={(e) => set("backgroundUsd", e.target.value)} />
        <Field label="Background work, in tokens" inputMode="numeric" value={draft.backgroundTokens} disabled={!host} {...errorOf(errors.background_tokens_daily)} onChange={(e) => set("backgroundTokens", e.target.value)} />
      </fieldset>
      <fieldset className="learn-fieldset">
        <legend>Models and the graph</legend>
        <Field label="Teaching model" placeholder="First ready subscription model" value={draft.strong} disabled={refusal !== null} onChange={(e) => set("strong", e.target.value)} />
        <Field label="Light model" placeholder="First ready subscription model" value={draft.cheap} disabled={refusal !== null} onChange={(e) => set("cheap", e.target.value)} />
        <Toggle label="Add sessions to the knowledge graph automatically" hint="Off means you choose when, from a session" checked={draft.extraction === "auto"} disabled={refusal !== null} onChange={(v) => set("extraction", v ? "auto" : "manual")} />
      </fieldset>
      {failure !== null && <p className="learn-callout" role="alert">{failure}</p>}
      <div className="learn-actions">
        <Button variant="primary" type="submit" disabled={!dirty || saving || Object.keys(errors).length > 0}>{saving ? "Saving…" : "Save changes"}</Button>
      </div>
    </form>
  );
}

function Spend({ status }: { status: aether.NoesisStatus }) {
  const budget = status.settings.budget;
  const background = status.spend_today.background;
  const rows = [
    { label: "Teaching", spent: spentUsd(status.spend_today.interactive), budget: budget.interactive_usd_daily },
    { label: "Background", spent: spentUsd(background), budget: budget.background_usd_daily },
  ];
  const usedTokens = background.input_tokens + background.output_tokens;
  return (
    <section className="learn-spend" aria-label="Spend today">
      <h3 className="learn-subtitle">Spent today</h3>
      {rows.map((row) => (
        <div key={row.label} className="learn-meter-row">
          <span>{row.label}</span>
          <meter className="learn-meter" min={0} max={1} value={budgetShare(row.spent, row.budget)} aria-label={`${row.label} spend`} />
          <span className="learn-num">{usd(row.spent)} of {usd(row.budget)}</span>
        </div>
      ))}
      <p className="learn-muted">Background tokens: {usedTokens.toLocaleString("en")} of {budget.background_tokens_daily.toLocaleString("en")}. Subscription use counts at its estimated price.</p>
    </section>
  );
}

function Toggle({ label, hint, checked, disabled, onChange }: { label: string; hint?: string; checked: boolean; disabled?: boolean; onChange: (value: boolean) => void }) {
  return (
    <label className="learn-toggle">
      <input type="checkbox" role="switch" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
      <span className="learn-toggle-text"><span>{label}</span>{hint && <span className="learn-muted">{hint}</span>}</span>
    </label>
  );
}

function errorOf(error: string | undefined): { error?: string } {
  return error === undefined ? {} : { error };
}
