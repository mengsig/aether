import { selectConnections, useSelector } from "@aether-ui/core";
import type { aether, archon } from "@aether-ui/protocol";
import { Button, Dialog, Field } from "@aether-ui/ui";
import { useState, type FormEvent } from "react";
import { useChange, useRead, type ArchonAccess } from "../queue/archon.ts";
import { invariant } from "../queue/model.ts";

const EFFORTS: archon.Effort[] = ["low", "medium", "high", "x_high", "max"];
const nameOf = (model: archon.ModelRef | aether.ModelRef): string => typeof model === "string" ? model :
  typeof model.provider === "string" && typeof model.model === "string" ? `${model.provider}/${model.model}` : "Unknown model";

function availableModels(models: aether.ModelInfo[], policy: archon.ModelPolicy): aether.ModelInfo[] {
  const allow = policy.allow?.map(nameOf);
  const deny = policy.deny.map(nameOf);
  return models.filter((model) => model.configured && (!allow || allow.includes(nameOf(model.model))) && !deny.includes(nameOf(model.model)));
}

function Settings({ item }: { readonly item: archon.QueueItem }) {
  const stages = Object.entries(item.model_plan);
  return <div aria-label="Current item settings">
    <p>Budget: {item.budget_usd == null ? "Not set" : `$${item.budget_usd}`}</p>
    <p>Stage timeouts: {Object.entries(item.stage_timeouts ?? {}).map(([stage, seconds]) => `${stage}: ${seconds}s`).join(", ") || "Defaults"}</p>
    {stages.length ? stages.map(([stage, choices]) => <p key={stage}>{stage}: {choices.map((choice) => `${nameOf(choice.model)} · ${choice.effort}`).join(", ") || "No model selected"}</p>) : <p>Model plan: Not available</p>}
    <p className="item-muted">For a running item, its run page shows recorded plan settings and attempts.</p>
  </div>;
}

interface Draft { stage: string; model: string; effort: string; note: string; budget: string; minutes: string }
const EMPTY: Draft = { stage: "", model: "", effort: "", note: "", budget: "", minutes: "" };

function steerFrom(draft: Draft): archon.Steer {
  return { ...(draft.stage && { stage: draft.stage }), ...(draft.model && { model: draft.model }),
    ...(draft.effort && { effort: draft.effort as archon.Effort }), ...(draft.note.trim() && { note: draft.note.trim() }),
    ...(draft.budget && { budget_usd: Number(draft.budget) }), ...(draft.minutes && { extend_minutes: Number(draft.minutes) }) };
}

function validation(draft: Draft): string | null {
  if (draft.budget && (!Number.isFinite(Number(draft.budget)) || Number(draft.budget) <= 0)) return "Budget must be a positive amount.";
  if (draft.minutes && (!Number.isSafeInteger(Number(draft.minutes)) || Number(draft.minutes) <= 0)) return "Extra time must be a positive whole number of minutes.";
  if (!draft.model && !draft.effort && !draft.note.trim() && !draft.budget && !draft.minutes) return "Choose a change or write a note.";
  return null;
}

function queuedPlan(item: archon.QueueItem, draft: Draft): archon.ModelPlanDraft | null {
  if (item.state.state !== "queued" || (!draft.model && !draft.effort)) return null;
  const selected = item.model_plan[draft.stage];
  invariant(selected !== undefined && selected.length > 0, "a queued plan edit names a populated stage");
  return Object.fromEntries(Object.entries(item.model_plan).map(([stage, choices]) => [stage, choices.map((choice) => {
    const model = stage === draft.stage && draft.model ? draft.model : null;
    const source = !model && choice.tier ? { tier: choice.tier.requested, provider: choice.tier.provider ?? null } : { model: model ?? choice.model };
    return { ...source, effort: stage === draft.stage && draft.effort ? draft.effort as archon.Effort : choice.effort };
  })]));
}

interface FormProps { readonly item: archon.QueueItem; readonly draft: Draft; readonly set: (patch: Partial<Draft>) => void; readonly choices: aether.ModelInfo[]; readonly efforts: archon.Effort[]; readonly policyReady: boolean }
function Controls({ item, draft, set, choices, efforts, policyReady }: FormProps) {
  return <>
    <label className="queue-select"><span className="ui-field-label">Stage</span><select className="ui-field-input" value={draft.stage} onChange={(e) => set({ stage: e.target.value })}>
      <option value="">Current or next stage</option>{Object.keys(item.model_plan).map((stage) => <option key={stage} value={stage}>{stage}</option>)}
    </select></label>
    <label className="queue-select"><span className="ui-field-label">Model</span><select className="ui-field-input" value={draft.model} onChange={(e) => set({ model: e.target.value })}>
      <option value="">Keep current</option>{choices.map((model) => <option key={nameOf(model.model)} value={nameOf(model.model)}>{model.name} ({nameOf(model.model)})</option>)}
    </select></label>
    <label className="queue-select"><span className="ui-field-label">Effort</span><select className="ui-field-input" value={draft.effort} onChange={(e) => set({ effort: e.target.value })} disabled={!policyReady}>
      <option value="">Keep current</option>{efforts.map((effort) => <option key={effort} value={effort}>{effort}</option>)}
    </select></label>
    <label className="queue-select"><span className="ui-field-label">Note</span><textarea className="ui-field-input" rows={3} value={draft.note} onChange={(e) => set({ note: e.target.value })} /></label>
    <Field label="Budget (USD)" type="number" min="0" step="any" value={draft.budget} onChange={(e) => set({ budget: e.target.value })} />
    <Field label="Extra time (minutes)" type="number" min="1" step="1" disabled={item.state.state !== "running"} value={draft.minutes} onChange={(e) => set({ minutes: e.target.value })} />
    {item.state.state !== "running" ? <p className="item-muted">Extra time can be added once a stage is running.</p> : null}
  </>;
}

export function SteerItem({ access, item, active, disabled, policy, onDone }: { readonly access: ArchonAccess; readonly item: archon.QueueItem; readonly active: boolean; readonly disabled: boolean; readonly policy: archon.ModelPolicy | undefined; readonly onDone: () => void }) {
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState<Draft>(EMPTY);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [aetherId, setAetherId] = useState("");
  const connections = useSelector(selectConnections);
  const aethers = Object.values(connections).filter((connection) => connection.daemon === "aether" && connection.state === "ready");
  const aetherConnection = aethers.length === 1 ? aethers[0] : aethers.find((connection) => connection.id === aetherId);
  const modelAccess: ArchonAccess = { ...access, conn: aetherConnection?.id ?? "", ready: Boolean(aetherConnection) };
  const models = useRead<aether.ModelInfo[]>(modelAccess, open && aetherConnection ? { cmd: "list_models" } : null, ["models"]);
  const choices = models.data && policy ? availableModels(models.data, policy) : [];
  const max = policy?.max_effort === "xhigh" ? "x_high" : policy?.max_effort;
  const efforts = EFFORTS.slice(0, max ? EFFORTS.indexOf(max) + 1 : EFFORTS.length);
  const set = (patch: Partial<Draft>) => setDraft((current) => ({ ...current, ...patch }));
  const close = () => { setOpen(false); setDraft(EMPTY); setError(null); };
  const change = useChange(access);
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const invalid = validation(draft);
    if (invalid) return setError(invalid);
    if (draft.model && !choices.some((model) => nameOf(model.model) === draft.model)) return setError("Choose an available model.");
    if (item.state.state === "queued" && (draft.model || draft.effort) && !item.model_plan[draft.stage]?.length) {
      return setError("Choose a stage with a model plan to update.");
    }
    invariant(item.id.length > 0, "a steer names an item");
    const plan = queuedPlan(item, draft);
    const steer = steerFrom(plan ? { ...draft, model: "", effort: "" } : draft);
    invariant(plan !== null || Object.keys(steer).some((key) => key !== "stage"), "an update changes something");
    setBusy(true);
    if (plan) {
      const update = await change({ cmd: "update_item", item: item.id, patch: { model_plan: plan } });
      if (!update.ok) { setBusy(false); return setError(update.error); }
      setDraft((current) => ({ ...current, model: "", effort: "" }));
      onDone();
    }
    const result = Object.keys(steer).some((key) => key !== "stage")
      ? await change({ cmd: "steer_item", item: item.id, steer }) : { ok: true as const };
    setBusy(false);
    if (!result.ok) return setError(result.error);
    close(); onDone();
  };
  return <><Settings item={item} />{active ? <Button disabled={disabled} title={access.refusal ?? (!access.live ? "Steering requires protocol 24" : undefined)} onClick={() => setOpen(true)}>Steer item</Button> : null}
    <Dialog open={open} onClose={close} title="Steer item" description="Notes reach active work; model and effort changes may apply to future stages. For a queued stage they update every parallel slot.">
      <form className="queue-form" onSubmit={(event) => void submit(event)} noValidate>
        {aethers.length > 1 ? <label className="queue-select"><span className="ui-field-label">Aether connection</span>
          <select className="ui-field-input" value={aetherId} onChange={(event) => { setAetherId(event.target.value); set({ model: "" }); }}>
            <option value="">Choose a connection</option>{aethers.map((connection) => <option key={connection.id} value={connection.id}>{connection.label}</option>)}
          </select></label> : null}
        <Controls item={item} draft={draft} set={set} choices={choices} efforts={efforts} policyReady={Boolean(policy)} />
        {!aetherConnection ? <p>{aethers.length > 1 ? "Choose an Aether connection to see its models." : "Aether is not connected; model choices are unavailable."}</p> : null}
        {models.error ? <p role="alert">Model choices unavailable: {models.error}</p> : null}
        {error ? <p role="alert" className="queue-error">{error}</p> : null}
        <div className="queue-form-actions"><Button onClick={close}>Cancel</Button><Button type="submit" variant="primary" disabled={busy || disabled}>Apply changes</Button></div>
      </form>
    </Dialog></>;
}
