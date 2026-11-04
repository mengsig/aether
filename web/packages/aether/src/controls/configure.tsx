// Model, effort, limits and identity for one agent, each applied with `update_agent`. Drafts hold
// only what the person typed; the current values shown come from core's store.

import { useReread } from "@aether-ui/core";
import type { ConnId } from "@aether-ui/protocol";
import { aether } from "@aether-ui/protocol";
import { Field } from "@aether-ui/ui";
import { useState } from "react";
import { invariant } from "../fleet/invariant.ts";
import { budgetPatch, DraftError, EFFORTS, modelRef, type BudgetDraft } from "./commands.ts";
import { Action, isList, ReadView, RunNote, Section, useRun, type Run } from "./kit.tsx";

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
  const [refresh, setRefresh] = useState(0);
  const models = useReread(conn, "models", aether.listModels(), ["models"], refresh);
  const [model, setModel] = useState("");
  const [effort, setEffort] = useState("");
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
      <ReadView entry={models} what="the models" empty="No models are configured" guard={isList} isEmpty={(list) => list.length === 0} retry={() => setRefresh((n) => n + 1)}>
        {(list) => (
          <div className="b3-form">
            <label className="b3-field">
              <span className="ui-field-label">Model</span>
              <select className="b3-select" value={model} onChange={(event) => setModel(event.target.value)}>
                <option value="">Keep {refText(agent.model)}</option>
                {(list as readonly aether.ModelInfo[]).filter((m) => m.configured).map((m) => (
                  <option key={refText(m.model)} value={refText(m.model)}>{m.name} · {m.provider_name}{m.is_default ? " (default)" : ""}</option>
                ))}
              </select>
            </label>
            <label className="b3-field">
              <span className="ui-field-label">Effort</span>
              <select className="b3-select" value={effort} onChange={(event) => setEffort(event.target.value)}>
                <option value="">Keep {agent.config.effort ?? "default"}</option>
                {EFFORTS.map((e) => <option key={e} value={e}>{e.replace("_", " ")}</option>)}
              </select>
            </label>
            <Action conn={conn} command={{ cmd: "update_agent", id: agent.id, patch: {} }} label="Apply model" busy={run.status.state === "busy"}
              onRun={() => apply(run, agent.id, build, setProblem, "Model updated")} />
          </div>
        )}
      </ReadView>
      {problem ? <p className="b3-note" role="alert" data-state="failed">{problem}</p> : null}
    </Section>
  );
}

const EMPTY_BUDGET: BudgetDraft = { cost: "", tokens: "", turns: "" };

function BudgetSection({ conn, agent, run }: { readonly conn: ConnId; readonly agent: Agent; readonly run: Run }) {
  const [draft, setDraft] = useState<BudgetDraft>(EMPTY_BUDGET);
  const [problem, setProblem] = useState<string | null>(null);
  const config = agent.config;
  const now = (value: number | null | undefined) => (value == null ? "No limit" : String(value));
  const edit = (field: keyof BudgetDraft) => (event: { target: { value: string } }) => setDraft((old) => ({ ...old, [field]: event.target.value }));
  return (
    <Section title="Limits" hint="Leave a field blank to keep its current limit.">
      <div className="b3-form">
        <Field label="Cost budget (USD)" inputMode="decimal" placeholder={now(config.cost_budget)} value={draft.cost} onChange={edit("cost")} />
        <Field label="Token budget" inputMode="numeric" placeholder={now(config.token_budget)} value={draft.tokens} onChange={edit("tokens")} />
        <Field label="Turn limit" inputMode="numeric" placeholder={now(config.max_turns)} value={draft.turns} onChange={edit("turns")} />
        <Action conn={conn} command={{ cmd: "update_agent", id: agent.id, patch: {} }} label="Apply limits" busy={run.status.state === "busy"}
          onRun={async () => {
            const reply = await apply(run, agent.id, () => budgetPatch(draft), setProblem, "Limits updated");
            if (reply) setDraft(EMPTY_BUDGET);
          }} />
      </div>
      {problem ? <p className="b3-note" role="alert" data-state="failed">{problem}</p> : null}
    </Section>
  );
}

function IdentitySection({ conn, agent, run }: { readonly conn: ConnId; readonly agent: Agent; readonly run: Run }) {
  invariant(agent.id.trim() !== "", "identity changes name their agent");
  invariant(conn.trim() !== "", "identity changes target a daemon connection");
  const [name, setName] = useState("");
  const [instructions, setInstructions] = useState("");
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
        <Field label="New name" placeholder={agent.name} value={name} onChange={(event) => setName(event.target.value)} />
        <Action conn={conn} command={{ cmd: "update_agent", id: agent.id, patch: {} }} label="Rename" busy={run.status.state === "busy"}
          onRun={async () => { if (await apply(run, agent.id, named, setProblem, "Renamed")) setName(""); }} />
      </div>
      {current === "" ? <p className="b3-meta">No standing instructions yet.</p> : <p className="b3-prose b3-quote">{current}</p>}
      <label className="b3-field">
        <span className="ui-field-label">New instructions</span>
        <textarea className="b3-textarea" rows={4} value={instructions} onChange={(event) => setInstructions(event.target.value)} />
      </label>
      <div className="b3-row">
        <Action conn={conn} command={{ cmd: "update_agent", id: agent.id, patch: {} }} label="Replace instructions" busy={run.status.state === "busy"}
          confirm={{ title: "Replace the instructions?", body: "The agent works from the new instructions from its next turn; the old ones are not kept here.", confirmLabel: "Replace" }}
          onRun={async () => { if (await apply(run, agent.id, instructed, setProblem, "Instructions replaced")) setInstructions(""); }} />
        {current === "" ? null : (
          <Action conn={conn} command={clear} label="Clear instructions" variant="danger" busy={run.status.state === "busy"}
            confirm={{ title: "Clear the instructions?", body: "The agent will have no standing instructions from its next turn.", confirmLabel: "Clear instructions" }}
            onRun={async () => {
              if (await run.send(clear, "Instructions cleared")) { setInstructions(""); setProblem(null); }
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
