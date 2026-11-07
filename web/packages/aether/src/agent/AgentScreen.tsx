// `#/agents/:conn/:agent`: one agent's identity, state, model, cost and budget, above its
// conversation. Everything shown is derived from core's store; this screen keeps no daemon data.

import { routeHref, selectAgent, selectAttention, selectConnection, useSelector } from "@aether-ui/core";
import type { aether, ConnId, RouteProps } from "@aether-ui/protocol";
import { Badge, EmptyState, EntityActionMenu, Skeleton, StatusPill } from "@aether-ui/ui";
import { useMemo } from "react";
import { Conversation } from "../conversation/Conversation.tsx";
import { agentStatus } from "../fleet/rows.ts";
import { roleLabel } from "../fleet/roles.ts";
import { invariant } from "../fleet/invariant.ts";
import "./agent.css";
import { agentFacts, statusDetail } from "./facts.ts";

function AgentLink({ conn, id, children }: { readonly conn: ConnId; readonly id: string; readonly children: string }) {
  return <a className="ui-link" href={routeHref("#/agents/:conn/:agent", { conn, agent: id })}>{children}</a>;
}

function AgentHeader({ conn, agent }: { readonly conn: ConnId; readonly agent: aether.Agent }) {
  const status = agentStatus(agent);
  const attention = useSelector(useMemo(() => selectAttention(conn), [conn]));
  const waitingApproval = attention?.approvals.some(request => request.agent_id === agent.id && request.status.state === "pending") ?? false;
  const role = roleLabel(agent.role);
  const parent = useSelector(useMemo(() => selectAgent(conn, agent.parent ?? ""), [conn, agent.parent]));
  const detail = statusDetail(agent.status);
  return (
    <header className="agent-head">
      <div className="agent-title">
        <h2 className="ui-page-title agent-name">{agent.name}</h2>
        <StatusPill status={waitingApproval ? "blocked" : status.status} label={waitingApproval ? "Waiting for your approval" : status.label} />
        {agent.paused ? <Badge tone="warning">Paused</Badge> : null}
        {agent.archived ? <Badge>Archived</Badge> : null}
        <EntityActionMenu entity={{ kind: "agent", conn, id: agent.id }} />
      </div>
      <p className="agent-sub">
        <span title={agent.role}>{role.kind === "agent" ? role.text || "No role" : <Badge tone="accent">{role.text}</Badge>}</span>
        <code className="agent-id">{agent.id}</code>
        {agent.parent ? <span>Delegated by <AgentLink conn={conn} id={agent.parent}>{parent?.name ?? agent.parent}</AgentLink></span> : null}
      </p>
      {waitingApproval ? <p className="agent-detail"><a className="ui-link" href="#/inbox">Review approval in Inbox</a></p> : detail ? <p className="agent-detail" data-state={status.state}>{detail}</p> : null}
    </header>
  );
}

function AgentFacts({ agent }: { readonly agent: aether.Agent }) {
  const facts = useMemo(() => agentFacts(agent), [agent]);
  return (
    <dl className="ui-card agent-facts">
      {facts.map((fact) => (
        <div key={fact.label} className="agent-fact">
          <dt>{fact.label}</dt>
          <dd title={fact.hint}>{fact.value}{fact.hint ? <span className="agent-hint">{fact.hint}</span> : null}</dd>
        </div>
      ))}
    </dl>
  );
}

export function AgentScreen({ conn, id }: { readonly conn: ConnId; readonly id: string }) {
  invariant(conn !== "" && id !== "", "the agent route names a connection and an agent");
  const info = useSelector(useMemo(() => selectConnection(conn), [conn]));
  const agent = useSelector(useMemo(() => selectAgent(conn, id), [conn, id]));
  if (!info) {
    return <EmptyState title="Not connected">No connection {conn} is open. <a className="ui-link" href="#/fleet">Back to the fleet</a></EmptyState>;
  }
  if (!agent && info.state !== "ready") {
    return <section className="ui-page agent" aria-busy="true" aria-label="Loading the agent"><Skeleton height="3rem" /><Skeleton height="8rem" /></section>;
  }
  if (!agent) {
    return <EmptyState title="No such agent">{info.label} has no agent {id}. <a className="ui-link" href="#/fleet">Back to the fleet</a></EmptyState>;
  }
  return (
    <section className="ui-page agent" aria-label={`Agent ${agent.name}`}>
      <AgentHeader conn={conn} agent={agent} />
      <AgentFacts agent={agent} />
      <Conversation key={`${conn}/${id}`} conn={conn} agent={id} />
    </section>
  );
}

/** The route's screen: `:conn` and `:agent` come from the hash (CONTRACTS.md §2.3). */
export default function AgentRoute({ params }: RouteProps) {
  const conn = params.conn ?? "";
  const agent = params.agent ?? "";
  if (conn === "" || agent === "") return <EmptyState title="No agent named">The address names no connection or agent.</EmptyState>;
  return <AgentScreen conn={conn} id={agent} />;
}
