// Approvals for one agent: which side effects need a person's approval (`set_approvals`), and the
// standing rules that approve or deny a tool call without asking (list, add, revoke).

import { useReread } from "@aether-ui/core";
import type { ConnId } from "@aether-ui/protocol";
import { aether } from "@aether-ui/protocol";
import { Badge, Field } from "@aether-ui/ui";
import { useState } from "react";
import { invariant } from "../fleet/invariant.ts";
import { DraftError, ruleCommand, SIDE_EFFECTS, type RuleDraft } from "./commands.ts";
import { Action, isList, ReadView, RunNote, Section, useRun, type Confirm, type Run } from "./kit.tsx";

const EFFECT_LABELS: Readonly<Record<aether.SideEffectClass, string>> = {
  read_only: "Reads", idempotent_mutation: "Repeatable changes", mutation: "Changes", external: "External effects",
};

function PolicySection({ conn, agent, run }: { readonly conn: ConnId; readonly agent: aether.Agent; readonly run: Run }) {
  const current = agent.config.approval?.require_for ?? [];
  const [draft, setDraft] = useState<ReadonlySet<aether.SideEffectClass> | null>(null);
  const [subtree, setSubtree] = useState(false);
  const chosen = draft ?? new Set(current);
  const toggle = (effect: aether.SideEffectClass) => {
    const next = new Set(chosen);
    if (next.has(effect)) next.delete(effect);
    else next.add(effect);
    setDraft(next);
  };
  const command = aether.setApprovals({ agent: agent.id, policy: { require_for: SIDE_EFFECTS.filter((e) => chosen.has(e)) }, ...(subtree ? { subtree } : {}) });
  return (
    <Section title="Ask before" hint="The agent waits for a person before any tool call with a checked effect.">
      <div className="b3-row">
        {SIDE_EFFECTS.map((effect) => (
          <label key={effect} className="b3-check">
            <input type="checkbox" checked={chosen.has(effect)} onChange={() => toggle(effect)} />
            <span>{EFFECT_LABELS[effect]}</span>
          </label>
        ))}
      </div>
      <div className="b3-row">
        <label className="b3-check">
          <input type="checkbox" checked={subtree} onChange={(event) => setSubtree(event.target.checked)} />
          <span>Also for the agents it delegated to</span>
        </label>
        <Action conn={conn} command={command} label="Apply approval policy" busy={run.status.state === "busy"} blocked={draft === null && !subtree ? "Nothing changed yet" : null}
          onRun={async () => { if (await run.send(command, "Approval policy applied")) setDraft(null); }} />
      </div>
    </Section>
  );
}

function scopeLabel(scope: aether.RuleScope, agent: string): string {
  invariant(agent.trim() !== "", "a rule's scope is shown relative to an agent");
  invariant(["everywhere", "agent", "subtree", "run"].includes(scope.scope), "a stored rule has a supported scope");
  if (scope.scope === "everywhere") return "Everywhere";
  if (scope.scope === "agent") return scope.id === agent ? "This agent" : `Agent ${scope.id}`;
  if (scope.scope === "subtree") return scope.root === agent ? "This agent and its delegates" : `Delegates of ${scope.root}`;
  return `Run ${scope.id}`;
}

function RuleRow({ conn, agent, rule, run, changed }: { readonly conn: ConnId; readonly agent: string; readonly rule: aether.ApprovalRule; readonly run: Run; readonly changed: () => void }) {
  const match = [rule.match.tool, rule.match.argument, rule.match.resource_prefix].filter(Boolean).join(" · ") || "Any tool call";
  const revoke = (reason: string) => aether.revokeApprovalRule({ id: rule.id, ...(reason ? { reason } : {}) });
  return (
    <li className="b3-item">
      <div className="b3-item-head">
        <p className="b3-item-title"><span className="b3-code">{match}</span></p>
        <Badge tone={rule.decision === "approved" ? "success" : "danger"}>{rule.decision === "approved" ? "Approves" : "Denies"}</Badge>
      </div>
      <p className="b3-meta">{scopeLabel(rule.scope, agent)}{rule.note ? ` · ${rule.note}` : ""}</p>
      <div className="b3-row">
        <Action conn={conn} command={revoke("")} label="Revoke" variant="danger" busy={run.status.state === "busy"}
          confirm={{ title: "Revoke this rule?", body: `Tool calls matching ${match} will ask again.`, confirmLabel: "Revoke rule", reason: "optional" }}
          onRun={async (reason) => { if (await run.send(revoke(reason), "Rule revoked")) changed(); }} />
      </div>
    </li>
  );
}

const EMPTY_RULE: RuleDraft = { tool: "", argument: "", resource: "", note: "", decision: "approved", scope: "agent" };

function RuleFields({ draft, edit, runId }: { readonly draft: RuleDraft; readonly edit: (patch: Partial<RuleDraft>) => void; readonly runId: string }) {
  invariant(["agent", "subtree", "run"].includes(draft.scope), "the rule form selects a browser scope");
  invariant(draft.decision === "approved" || draft.decision === "denied", "the rule form selects an approval decision");
  return (
    <div className="b3-form">
      <Field label="Tool" hint="Optional; blank matches any tool." placeholder="shell" value={draft.tool} onChange={(event) => edit({ tool: event.target.value })} />
      <Field label="Argument (optional)" placeholder="git status" value={draft.argument} onChange={(event) => edit({ argument: event.target.value })} />
      <Field label="Resource prefix (optional)" value={draft.resource} onChange={(event) => edit({ resource: event.target.value })} />
      <Field label="Note (optional)" value={draft.note} onChange={(event) => edit({ note: event.target.value })} />
      <label className="b3-field">
        <span className="ui-field-label">Scope</span>
        <select className="b3-select" value={draft.scope} onChange={(event) => edit({ scope: event.target.value as RuleDraft["scope"] })}>
          <option value="agent">This agent</option>
          <option value="subtree">This agent and its delegates</option>
          <option value="run" disabled={runId.trim() === ""}>This run{runId.trim() === "" ? " (this agent has no run)" : ""}</option>
        </select>
      </label>
      <label className="b3-field">
        <span className="ui-field-label">Decision</span>
        <select className="b3-select" value={draft.decision} onChange={(event) => edit({ decision: event.target.value as aether.ApprovalDecision })}>
          <option value="approved">Approve without asking</option>
          <option value="denied">Deny without asking</option>
        </select>
      </label>
    </div>
  );
}

const SCOPE_TEXT: Readonly<Record<RuleDraft["scope"], string>> = {
  agent: "this agent", subtree: "this agent and the agents it delegates to", run: "every agent in this run",
};

/** The confirm step says exactly what the rule will answer; an unrestricted approval is called out. */
function ruleConfirm(draft: RuleDraft): Confirm {
  invariant(draft.decision === "approved" || draft.decision === "denied", "a rule approves or denies");
  invariant(draft.scope in SCOPE_TEXT, "a rule has a known scope");
  const tool = draft.tool.trim(), argument = draft.argument.trim(), resource = draft.resource.trim();
  const matches = [tool ? `tool ${tool}` : "any tool", argument ? `argument "${argument}"` : "", resource ? `resources under ${resource}` : ""].filter(Boolean).join(", ");
  const verb = draft.decision === "approved" ? "approves" : "denies";
  const everything = draft.decision === "approved" && !tool && !argument && !resource;
  const warning = everything ? " It matches every tool call, so nothing in this scope asks for approval until the rule is revoked." : "";
  return {
    title: draft.decision === "approved" ? "Approve matching calls without asking?" : "Deny matching calls without asking?",
    body: `The rule ${verb} calls to ${matches} for ${SCOPE_TEXT[draft.scope]}.${warning}`,
    confirmLabel: "Add rule",
  };
}

function NewRule({ conn, agent, run, changed }: { readonly conn: ConnId; readonly agent: aether.Agent; readonly run: Run; readonly changed: () => void }) {
  invariant(agent.id.trim() !== "", "a new rule names its agent");
  invariant(conn.trim() !== "", "a new rule targets a daemon connection");
  const [draft, setDraft] = useState<RuleDraft>(EMPTY_RULE);
  const [problem, setProblem] = useState<string | null>(null);
  const scope: aether.RuleScope = draft.scope === "subtree" ? { scope: "subtree", root: agent.id }
    : { scope: draft.scope, id: draft.scope === "run" ? agent.run : agent.id };
  const add = async () => {
    try {
      const command = ruleCommand(agent.id, { ...draft, run: agent.run });
      setProblem(null);
      if (await run.send(command, "Rule added")) {
        setDraft((old) => ({ ...old, tool: "", argument: "", resource: "", note: "" }));
        changed();
      }
    } catch (error) {
      if (!(error instanceof DraftError)) throw error;
      setProblem(error.message);
    }
  };
  return (
    <div className="b3-stack">
      <RuleFields draft={draft} edit={(patch) => setDraft((old) => ({ ...old, ...patch }))} runId={agent.run ?? ""} />
      <div className="b3-row">
        <Action conn={conn} command={{ cmd: "add_approval_rule", scope }} label="Add rule" busy={run.status.state === "busy"} onRun={add}
          confirm={ruleConfirm(draft)} variant={draft.decision === "approved" && !draft.tool.trim() && !draft.argument.trim() && !draft.resource.trim() ? "danger" : "secondary"} />
      </div>
      {problem ? <p className="b3-note" role="alert" data-state="failed">{problem}</p> : null}
    </div>
  );
}

export function ApprovalsPanel({ conn, agent }: { readonly conn: ConnId; readonly agent: aether.Agent }) {
  invariant(agent.id !== "", "approval controls name their agent");
  const run = useRun(conn);
  const [refresh, setRefresh] = useState(0);
  const changed = () => setRefresh((n) => n + 1);
  const rules = useReread(conn, "approval_rules", aether.listApprovalRules({}), ["approval_rules"], refresh);
  return (
    <>
      <PolicySection conn={conn} agent={agent} run={run} />
      <Section title="Standing rules" hint="Rules answer matching approval requests on their own, until revoked.">
        <ReadView entry={rules} what="the rules" empty="No standing rules" guard={isList} isEmpty={(list) => list.length === 0} retry={changed}>
          {(list) => (
            <ul className="b3-list" aria-label="Standing rules">
              {(list as readonly aether.ApprovalRule[]).filter((rule) => !rule.revoked).map((rule) => (
                <RuleRow key={rule.id} conn={conn} agent={agent.id} rule={rule} run={run} changed={changed} />
              ))}
            </ul>
          )}
        </ReadView>
        <NewRule conn={conn} agent={agent} run={run} changed={changed} />
      </Section>
      <RunNote status={run.status} />
    </>
  );
}
