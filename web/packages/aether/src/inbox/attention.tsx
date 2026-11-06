// What needs a person on one daemon, from core's synced attention: questions to answer, tool calls
// to approve or deny, and agents that failed or are blocked, with the way to their controls.

import { useDraft, routeHref, selectAgents, selectAttention, useSelector } from "@aether-ui/core";
import type { ConnId } from "@aether-ui/protocol";
import { aether } from "@aether-ui/protocol";
import { Badge, TextArea } from "@aether-ui/ui";
import { useMemo } from "react";
import { invariant } from "../fleet/invariant.ts";
import { Action, RunNote, Section, useRun } from "../controls/kit.tsx";

function when(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}

function Question({ conn, question }: { readonly conn: ConnId; readonly question: aether.PendingQuestion }) {
  const run = useRun(conn);
  const draft = useDraft(`inbox/${question.agent}/${question.question_seq}/answer`, { answer: "" });
  const { answer } = draft.value;
  const name = question.agent_name ?? question.agent;
  const command = aether.replyToQuestion({ to: question.agent, question_seq: question.question_seq, content: answer.trim() });
  return (
    <li className="b3-item">
      <div className="b3-item-head">
        <p className="b3-item-title">{name} asks</p>
        <span className="b3-meta">{when(question.asked_at)}</span>
      </div>
      <p className="b3-prose">{question.question}</p>
      <TextArea label={`Answer ${name}`} rows={2} value={answer} onChange={event => draft.update({ answer: event.target.value })} />
      <div className="b3-row">
        <Action conn={conn} command={command} label="Send answer" variant="primary" busy={run.status.state === "busy"} blocked={answer.trim() === "" ? "Write an answer first" : null}
          onRun={async () => { if (await run.send(command, "Answer sent")) draft.clear(); }} />
      </div>
      <RunNote status={run.status} />
    </li>
  );
}

function Approval({ conn, request, name }: { readonly conn: ConnId; readonly request: aether.ApprovalRequest; readonly name: string }) {
  const run = useRun(conn);
  const approve = aether.resolveApproval({ id: request.id, decision: "approved" });
  const deny = (note: string) => aether.resolveApproval({ id: request.id, decision: "denied", ...(note ? { note } : {}) });
  const busy = run.status.state === "busy";
  return (
    <li className="b3-item">
      <div className="b3-item-head">
        <p className="b3-item-title">{name} wants <span className="b3-code">{request.tool}</span></p>
        <Badge tone={request.side_effect === "external" ? "warning" : "neutral"}>{request.side_effect.replaceAll("_", " ")}</Badge>
      </div>
      <p className="b3-prose">{request.description}</p>
      {request.argument ? <pre className="b3-pre">{request.argument}</pre> : null}
      <div className="b3-row">
        <Action conn={conn} command={approve} label="Approve" variant="primary" busy={busy} onRun={() => run.send(approve, "Approved")} />
        <Action conn={conn} command={deny("")} label="Deny" busy={busy}
          confirm={{ title: `Deny ${request.tool}?`, body: `${name} is told the call was denied and carries on without it.`, confirmLabel: "Deny", reason: "optional" }}
          onRun={(note) => run.send(deny(note), "Denied")} />
      </div>
      <RunNote status={run.status} />
    </li>
  );
}

const KIND_LABELS: Readonly<Record<string, string>> = { agent_failed: "Failed", agent_blocked: "Blocked", task_failed: "Task failed", task_completed: "Task done" };

/** The attention of one daemon; empty sections are left out, and an empty daemon says it is clear. */
export function DaemonAttention({ conn, label }: { readonly conn: ConnId; readonly label: string }) {
  invariant(conn !== "", "attention belongs to a connection");
  const attention = useSelector(useMemo(() => selectAttention(conn), [conn]));
  const agents = useSelector(useMemo(() => selectAgents(conn), [conn]));
  if (attention === null) return <p className="b3-meta" aria-busy="true">Reading what {label} needs…</p>;
  const name = (id: string) => agents[id]?.name ?? id;
  const approvals = attention.approvals.filter((a) => (a.status as { state: string }).state === "pending");
  const flagged = attention.agents.filter((a) => a.kind !== "approval" && a.kind !== "question");
  if (attention.questions.length + approvals.length + flagged.length === 0) return <p className="b3-meta">Nothing on {label} needs you.</p>;
  return (
    <>
      {attention.questions.length > 0 ? (
        <Section title="Questions" hint={`Agents on ${label} waiting for an answer.`}>
          <ul className="b3-list">{attention.questions.map((q) => <Question key={`${q.agent}:${q.question_seq}`} conn={conn} question={q} />)}</ul>
        </Section>
      ) : null}
      {approvals.length > 0 ? (
        <Section title="Approvals" hint="Tool calls held until a person decides.">
          <ul className="b3-list">{approvals.map((a) => <Approval key={a.id} conn={conn} request={a} name={name(a.agent_id)} />)}</ul>
        </Section>
      ) : null}
      {flagged.length > 0 ? (
        <Section title="Agents" hint="Agents that stopped or are stuck.">
          <ul className="b3-list">
            {flagged.map((a) => (
              <li key={`${a.agent}:${a.kind}`} className="b3-item">
                <div className="b3-item-head">
                  <p className="b3-item-title">{a.agent_name ?? name(a.agent)}</p>
                  <Badge tone={a.kind.endsWith("failed") ? "danger" : "warning"}>{KIND_LABELS[a.kind] ?? a.kind}</Badge>
                </div>
                <p className="b3-prose">{a.reason}</p>
                <p className="b3-meta">
                  <a className="ui-link" href={routeHref("#/controls/:conn/:agent", { conn, agent: a.agent })}>Open controls</a>
                  {" · "}<a className="ui-link" href={routeHref("#/agents/:conn/:agent", { conn, agent: a.agent })}>Open the conversation</a>
                </p>
              </li>
            ))}
          </ul>
        </Section>
      ) : null}
    </>
  );
}
