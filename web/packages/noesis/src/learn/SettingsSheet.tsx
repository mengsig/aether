// Noesis settings with today's spend against its budgets (CONTRACTS.md §6.1). The switch, budgets and
// billing are the host's; other devices see them read-only. Saving sends only what changed.

import { useDraft, type Draft } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { Button, Field, ModelPicker, modelOptions, Sheet, Switch } from "@aether-ui/ui";
import { useMemo, useState, type FormEvent } from "react";
import { useGate, useHostAdmin, useNoesisConfigure, useNoesisRead, useOperateRefusal } from "./data.ts";
import { budgetShare, invariant, spentUsd, usd } from "./model.ts";
import { hasReadyModel, useModelChoices, type ModelChoices } from "./models.ts";
import { draftOf, patchOf, settingsDraftKey, type BudgetField, type SettingsDraft } from "./settings-draft.ts";

export const DEFAULT_MODEL_LABEL = "Default (first ready subscription model)";
const HOST_NOTE = "Only the daemon's own host can change this";

export function SettingsSheet({ open, conn, onClose }: { open: boolean; conn: string; onClose: () => void }) {
  const gate = useGate();
  // Spend moves all day: opening the sheet rereads the status, and nothing is read while it is closed.
  const read = useNoesisRead(conn, open ? { kind: "status" } : null);
  const status = gate.state === "on" || gate.state === "off" ? gate.status : null;
  return (
    <Sheet open={open} onClose={onClose} title="Noesis settings" description="Learning on this daemon, and what it may spend each day.">
      {read.error !== null && <p className="learn-callout" role="alert">Could not refresh the settings: {read.error}</p>}
      {status === null ? <p className="learn-muted">Loading settings…</p> : <SettingsForm conn={conn} status={status} onDone={onClose} />}
    </Sheet>
  );
}

/** The sheet's draft, seeded from the daemon's settings; the off state's "Turn on" writes the same draft. */
export function useSettingsDraft(conn: string, settings: aether.NoesisSettingsView): Draft<SettingsDraft> {
  invariant(conn.length > 0, "the settings draft belongs to a connection");
  const initial = useMemo(() => draftOf(settings), [settings]);
  return useDraft(settingsDraftKey(conn), initial);
}

function SettingsForm({ conn, status, onDone }: { conn: string; status: aether.NoesisStatus; onDone: () => void }) {
  const host = useHostAdmin(conn);
  const refusal = useOperateRefusal(conn);
  const configure = useNoesisConfigure(conn);
  const draft = useSettingsDraft(conn, status.settings);
  const value = draft.value;
  // Models are read only once Noesis is on, or about to be: while it stays off nothing else is read.
  const choices = useModelChoices(conn, status.settings.enabled || value.enabled);
  const [saving, setSaving] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const { patch, errors } = useMemo(() => patchOf(status.settings, value), [status.settings, value]);
  const dirty = Object.keys(patch).length > 0;
  const invalid = Object.keys(errors).length > 0;
  const set = (patch: Partial<SettingsDraft>) => draft.update(patch);
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!dirty || invalid) return;
    setSaving(true);
    const result = await configure(patch);
    setSaving(false);
    if (!result.ok) return setFailure(result.error);
    setFailure(null);
    draft.clear();
    onDone();
  };
  return (
    <form className="learn-settings" onSubmit={(event) => void submit(event)}>
      <Spend status={status} />
      <NoesisSwitches draft={value} host={host} set={set} />
      <BudgetFields draft={value} errors={errors} host={host} set={set} />
      <ModelFields draft={value} choices={choices} active={status.settings.enabled || value.enabled} disabled={refusal !== null} set={set} />
      {failure !== null && <p className="learn-callout" role="alert">{failure}</p>}
      <div className="learn-actions">
        <Button variant="primary" type="submit" data-testid="noesis-save" disabled={!dirty || saving || invalid}>{saving ? "Saving…" : "Save changes"}</Button>
        {draft.dirty && <Button variant="quiet" onClick={draft.clear}>Discard changes</Button>}
        {invalid && <span className="learn-muted" role="status">Fix the budgets above to save.</span>}
      </div>
    </form>
  );
}

interface SectionProps {
  readonly draft: SettingsDraft;
  readonly set: (patch: Partial<SettingsDraft>) => void;
}

function NoesisSwitches({ draft, host, set }: SectionProps & { host: boolean }) {
  return (
    <fieldset className="learn-fieldset">
      <legend>Noesis</legend>
      <Switch label="Turn on Noesis" data-testid="noesis-enable" checked={draft.enabled} disabled={!host} onChange={(enabled) => set({ enabled })}
        hint={host ? "Teaching, practice and the knowledge graph on this daemon" : HOST_NOTE} />
      <Switch label="Allow API-billed models" data-testid="noesis-allow-api" checked={draft.allowApi} disabled={!host} onChange={(allowApi) => set({ allowApi })}
        hint={host ? "Noesis spends your subscription by default. On lists models charged per request too." : HOST_NOTE} />
    </fieldset>
  );
}

const BUDGETS: readonly { readonly key: BudgetField; readonly field: "interactiveUsd" | "backgroundUsd" | "backgroundTokens"; readonly label: string; readonly testid: string; readonly hint?: string }[] = [
  { key: "interactive_usd_daily", field: "interactiveUsd", label: "Teaching, in dollars", testid: "noesis-budget-teaching", hint: "0 stops teaching for the day" },
  { key: "background_usd_daily", field: "backgroundUsd", label: "Background work, in dollars", testid: "noesis-budget-background" },
  { key: "background_tokens_daily", field: "backgroundTokens", label: "Background work, in tokens", testid: "noesis-budget-tokens" },
];

/** Each budget is checked as it is typed; the error sits under its field and keeps Save off. */
function BudgetFields({ draft, errors, host, set }: SectionProps & { errors: Partial<Record<BudgetField, string>>; host: boolean }) {
  return (
    <fieldset className="learn-fieldset">
      <legend>Daily budgets</legend>
      {BUDGETS.map((budget) => {
        const error = errors[budget.key];
        const hint = host ? budget.hint : HOST_NOTE;
        return (
          <Field key={budget.key} label={budget.label} data-testid={budget.testid} inputMode={budget.field === "backgroundTokens" ? "numeric" : "decimal"}
            value={draft[budget.field]} disabled={!host} {...(hint === undefined ? {} : { hint })} {...(error === undefined ? {} : { error })}
            onChange={(event) => set({ [budget.field]: event.target.value })} />
        );
      })}
    </fieldset>
  );
}

/** Why a saved model is not in the list: it is billed per request with that off, or not ready now. */
export function hiddenModelNote(ref: string, choices: ModelChoices, allowApi: boolean): string | undefined {
  if (ref === "" || !choices.loaded) return undefined;
  const listed = modelOptions(choices.models, choices.auth, { allowApiBilled: allowApi });
  if (listed.some((option) => option.id === ref && option.disabled !== true)) return undefined;
  invariant(ref.includes("/"), "a saved model is a provider/model ref");
  return `${ref} is not offered: it is billed per request or not ready. Choose another, or allow API-billed models.`;
}

function ModelFields({ draft, choices, active, disabled, set }: SectionProps & { choices: ModelChoices; active: boolean; disabled: boolean }) {
  const extraction = (
    <Switch label="Add sessions to the knowledge graph automatically" hint="Off means you choose when, from a session" checked={draft.extraction === "auto"}
      disabled={disabled} onChange={(on) => set({ extraction: on ? "auto" : "manual" })} />
  );
  if (!active) {
    return (
      <fieldset className="learn-fieldset">
        <legend>Models and the graph</legend>
        <p className="learn-muted">Turn Noesis on above to choose its models. Until then nothing is read from your providers.</p>
        {extraction}
      </fieldset>
    );
  }
  const pickers = [
    { field: "strong", label: "Teaching model", testid: "noesis-model-strong", hint: "Asks the questions and teaches." },
    { field: "cheap", label: "Light model", testid: "noesis-model-cheap", hint: "Grades answers and tidies the graph." },
  ] as const;
  return (
    <fieldset className="learn-fieldset">
      <legend>Models and the graph</legend>
      {choices.error !== null && <p className="learn-callout" role="alert">Could not list the models: {choices.error}</p>}
      {choices.loaded && !hasReadyModel(choices.models) && (
        <p className="learn-callout" role="status">No model is ready yet. <a className="ui-link" href="#/settings/providers">Sign in to a provider</a>, then come back.</p>
      )}
      {pickers.map((picker) => (
        <ModelPicker key={picker.field} label={picker.label} data-testid={picker.testid} models={choices.models} auth={choices.auth} value={draft[picker.field]}
          defaultLabel={DEFAULT_MODEL_LABEL} allowApiBilled={draft.allowApi} disabled={disabled}
          hint={hiddenModelNote(draft[picker.field], choices, draft.allowApi) ?? picker.hint} onChange={(ref) => set({ [picker.field]: ref })} />
      ))}
      {extraction}
    </fieldset>
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
