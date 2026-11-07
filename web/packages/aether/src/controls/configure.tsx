// Model, effort, limits and identity for one agent, each applied with `update_agent`. Drafts hold
// only what the person typed; the current values shown come from core's store.

import { useDraft } from "@aether-ui/core";
import type { ConnId } from "@aether-ui/protocol";
import { aether } from "@aether-ui/protocol";
import { Field, Segmented, TextArea } from "@aether-ui/ui";
import { useState } from "react";
import { Models } from "../activity/Models.tsx";
import { invariant } from "../fleet/invariant.ts";
import { budgetPatch, DraftError, EFFORTS, modelRef, type BudgetDraft } from "./commands.ts";
import { Action, RunNote, Section, useRun, type Run } from "./kit.tsx";

type Agent = aether.Agent;

const refText = (ref: aether.ModelRef): string => `${ref.provider}/${ref.model}`;

/** Builds the patch, or shows why the draft cannot be sent; a DraftError is the person's to fix. */
function apply(run: Run, id: string, build: () => aether.AgentPatch, setProblem: (text: string | null) => void, done: string): Promise<unknown> | undefined {
  try {
    const patch = build();
    setProblem(null);
    return run.send(aether.updateAgent({ id, patch }), done);
  } catch (error) {
    if (!(error instanceof DraftError)) throw error;
    setProblem(error.message);
    return undefined;
  }
}

function ModelSection({ conn, agent, run }: { readonly conn: ConnId; readonly agent: Agent; readonly run: Run }) {
  const draft = useDraft(`controls/${agent.id}/model`, { model: "", effort: "" });
  const { model, effort } = draft.value;
  const [problem, setProblem] = useState<string | null>(null);
  const build = () => {
    const patch: aether.AgentPatch = {};
    if (model !== "") patch.model = modelRef(model);
    if (effort !== "") patch.effort = effort as aether.Effort;
    if (Object.keys(patch).length === 0) throw new DraftError("Choose a model or an effort first");
    return patch;
  };
  return (
    <Section title="Model" hint={`Now ${refText(agent.model)}, effort ${agent.config.effort ?? "default"}. The change applies from its next turn.`}>
      <div className="b3-form">
        <Models conn={conn} screen="configure" value={model} onChange={model => draft.update({ model })}
          testId="configure-model" defaultLabel={`Keep ${refText(agent.model)}`} />
        <Segmented className="b3-span" label="Effort" value={effort} onChange={effort => draft.update({ effort })}
          options={[{ id: "", label: `Keep ${agent.config.effort ?? "default"}` }, ...EFFORTS.map(id => ({ id, label: id.replace("_", " ") }))]} />
        <Action conn={conn} command={{ cmd: "update_agent", id: agent.id, patch: {} }} label="Apply model" busy={run.status.state === "busy"}
          blocked={!model && !effort ? "Choose a model or an effort first" : null}
          onRun={async () => { if (await apply(run, agent.id, build, setProblem, "Model updated")) draft.clearIfUnchanged(); }} />
      </div>
      {problem ? <p className="b3-note" role="alert" data-state="failed">{problem}</p> : null}
    </Section>
  );
}

const EMPTY_BUDGET: BudgetDraft = { cost: "", tokens: "", turns: "" };

function BudgetSection({ conn, agent, run }: { readonly conn: ConnId; readonly agent: Agent; readonly run: Run }) {
  const form = useDraft(`controls/${agent.id}/budget`, { ...EMPTY_BUDGET });
  const draft = form.value;
  const validation = budgetProblem(draft);
  const [problem, setProblem] = useState<string | null>(null);
  const config = agent.config;
  const now = (value: number | null | undefined) => (value == null ? "No limit" : String(value));
  const edit = (field: keyof BudgetDraft) => (event: { target: { value: string } }) => form.update({ [field]: event.target.value });
  return (
    <Section title="Limits" hint="Leave a field blank to keep its current limit.">
      <div className="b3-form">
        <Field label="Cost budget (USD)" {...budgetFieldProps(draft.cost, "cost")} inputMode="decimal" placeholder={now(config.cost_budget)} value={draft.cost} onChange={edit("cost")} />
        <Field label="Token budget" {...budgetFieldProps(draft.tokens, "tokens")} inputMode="numeric" placeholder={now(config.token_budget)} value={draft.tokens} onChange={edit("tokens")} />
        <Field label="Turn limit" {...budgetFieldProps(draft.turns, "turns")} inputMode="numeric" placeholder={now(config.max_turns)} value={draft.turns} onChange={edit("turns")} />
        <Action conn={conn} command={{ cmd: "update_agent", id: agent.id, patch: {} }} label="Apply limits" blocked={validation} busy={run.status.state === "busy"}
          onRun={async () => {
            const reply = await apply(run, agent.id, () => budgetPatch(draft), setProblem, "Limits updated");
            if (reply) form.clearIfUnchanged();
          }} />
      </div>
      {problem ? <p className="b3-note" role="alert" data-state="failed">{problem}</p> : null}
    </Section>
  );
}

function IdentitySection({ conn, agent, run }: { readonly conn: ConnId; readonly agent: Agent; readonly run: Run }) {
  invariant(agent.id.trim() !== "", "identity changes name their agent");
  invariant(conn.trim() !== "", "identity changes target a daemon connection");
  const draft = useDraft(`controls/${agent.id}/identity`, { name: "", instructions: "" });
  const { name, instructions } = draft.value;
  const [problem, setProblem] = useState<string | null>(null);
  const current = agent.config.instructions ?? "";
  const clear = aether.updateAgent({ id: agent.id, patch: { instructions: "" } });
  const named = () => {
    if (name.trim() === "") throw new DraftError("Type the new name first");
    return { name: name.trim() };
  };
  const instructed = () => {
    if (instructions.trim() === "") throw new DraftError("Write the new instructions first");
    return { instructions: instructions.trim() };
  };
  return (
    <Section title="Identity" hint="Rename the agent, or replace the standing instructions it works from.">
      <div className="b3-form">
        <Field label="New name" placeholder={agent.name} value={name} onChange={(event) => draft.update({ name: event.target.value })} />
        <Action conn={conn} command={{ cmd: "update_agent", id: agent.id, patch: {} }} label="Rename" blocked={name.trim() ? null : "Type the new name first"} busy={run.status.state === "busy"}
          onRun={async () => { if (await apply(run, agent.id, named, setProblem, "Renamed")) draft.update({ name: "" }); }} />
      </div>
      {current === "" ? <p className="b3-meta">No standing instructions yet.</p> : <p className="b3-prose b3-quote">{current}</p>}
      <TextArea label="New instructions" rows={4} value={instructions} onChange={event => draft.update({ instructions: event.target.value })} />
      <div className="b3-row">
        <Action conn={conn} command={{ cmd: "update_agent", id: agent.id, patch: {} }} label="Replace instructions" blocked={instructions.trim() ? null : "Write the new instructions first"} busy={run.status.state === "busy"}
          confirm={{ title: "Replace the instructions?", body: "The agent works from the new instructions from its next turn; the old ones are not kept here.", confirmLabel: "Replace" }}
          onRun={async () => { if (await apply(run, agent.id, instructed, setProblem, "Instructions replaced")) draft.update({ instructions: "" }); }} />
        {current === "" ? null : (
          <Action conn={conn} command={clear} label="Clear instructions" variant="danger" busy={run.status.state === "busy"}
            confirm={{ title: "Clear the instructions?", body: "The agent will have no standing instructions from its next turn.", confirmLabel: "Clear instructions" }}
            onRun={async () => {
              if (await run.send(clear, "Instructions cleared")) { draft.update({ instructions: "" }); setProblem(null); }
            }} />
        )}
      </div>
      {problem ? <p className="b3-note" role="alert" data-state="failed">{problem}</p> : null}
    </Section>
  );
}

export function ConfigurePanel({ conn, agent }: { readonly conn: ConnId; readonly agent: Agent }) {
  invariant(agent.id !== "", "configuration names its agent");
  const run = useRun(conn);
  return (
    <>
      <ModelSection conn={conn} agent={agent} run={run} />
      <BudgetSection conn={conn} agent={agent} run={run} />
      <IdentitySection conn={conn} agent={agent} run={run} />
      <RunNote status={run.status} />
    </>
  );
}

function budgetProblem(draft: BudgetDraft): string | null {
  try { budgetPatch(draft); return null; }
  catch (error) { if (!(error instanceof DraftError)) throw error; return error.message; }
}

function budgetFieldProps(text: string, field: keyof BudgetDraft): { error?: string } {
  const error = text.trim() === "" ? null : budgetProblem({ ...EMPTY_BUDGET, [field]: text });
  return error ? { error } : {};
}
