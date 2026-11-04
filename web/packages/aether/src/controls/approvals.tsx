// Approvals for one agent: which side effects need a person's approval (`set_approvals`), and the
// standing rules that approve or deny a tool call without asking (list, add, revoke).

import { useReread } from "@aether-ui/core";
import type { ConnId } from "@aether-ui/protocol";
import { aether } from "@aether-ui/protocol";
import { Badge, Field } from "@aether-ui/ui";
import { useState } from "react";
import { invariant } from "../fleet/invariant.ts";
import { DraftError, ruleCommand, SIDE_EFFECTS } from "./commands.ts";
import { Action, isList, ReadView, RunNote, Section, useRun, type Run } from "./kit.tsx";

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
  const record = scope as { scope: string; id?: string; root?: string; run?: string };
  if (record.scope === "everywhere") return "Everywhere";
  if (record.scope === "agent") return record.id === agent ? "This agent" : `Agent ${record.id ?? ""}`;
  if (record.scope === "subtree") return record.root === agent ? "This agent and its delegates" : `Delegates of ${record.root ?? ""}`;
  return `Run ${record.run ?? ""}`;
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

function NewRule({ conn, agent, run, changed }: { readonly conn: ConnId; readonly agent: string; readonly run: Run; readonly changed: () => void }) {
  const [tool, setTool] = useState("");
  const [argument, setArgument] = useState("");
  const [decision, setDecision] = useState<aether.ApprovalDecision>("approved");
  const [subtree, setSubtree] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const add = async () => {
    try {
      const command = ruleCommand(agent, { tool, argument, decision, subtree });
      setProblem(null);
      if (await run.send(command, "Rule added")) { setTool(""); setArgument(""); changed(); }
    } catch (error) {
      if (!(error instanceof DraftError)) throw error;
      setProblem(error.message);
    }
  };
  return (
    <div className="b3-stack">
      <div className="b3-form">
        <Field label="Tool" placeholder="shell" value={tool} onChange={(event) => setTool(event.target.value)} />
        <Field label="Argument (optional)" placeholder="git status" value={argument} onChange={(event) => setArgument(event.target.value)} />
        <label className="b3-field">
          <span className="ui-field-label">Decision</span>
          <select className="b3-select" value={decision} onChange={(event) => setDecision(event.target.value as aether.ApprovalDecision)}>
            <option value="approved">Approve without asking</option>
            <option value="denied">Deny without asking</option>
          </select>
        </label>
      </div>
      <div className="b3-row">
        <label className="b3-check">
          <input type="checkbox" checked={subtree} onChange={(event) => setSubtree(event.target.checked)} />
          <span>Also for its delegates</span>
        </label>
        <Action conn={conn} command={{ cmd: "add_approval_rule", scope: { scope: "agent", id: agent } }} label="Add rule" busy={run.status.state === "busy"} onRun={add} />
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
        <NewRule conn={conn} agent={agent.id} run={run} changed={changed} />
      </Section>
      <RunNote status={run.status} />
    </>
  );
}
