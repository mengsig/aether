// `#/agents/:conn/:agent`: one agent's identity, state, model, cost and budget, above its
// chat. Everything shown is derived from core's store; this screen keeps no daemon data.

import { routeHref, selectAgent, selectAttention, selectConnection, useApp, useReread, useSelector } from "@aether-ui/core";
import { aether, type ConnId, type RouteProps } from "@aether-ui/protocol";
import { Badge, EmptyState, EntityActionMenu, ErrorState, Skeleton, StatusPill } from "@aether-ui/ui";
import { useEffect, useMemo, useState } from "react";
import { Chat } from "../conversation/Chat.tsx";
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
  const app = useApp();
  const info = useSelector(useMemo(() => selectConnection(conn), [conn]));
  const cached = useSelector(useMemo(() => selectAgent(conn, id), [conn, id]));
  const [refresh, setRefresh] = useState(0);
  const read = useReread(conn, `agent:${id}`, cached ? null : aether.getAgent({ id }), ["agent"], refresh);
  useEffect(() => {
    const value = read?.value;
    if (cached || value == null) return;
    invariant(typeof value === "object" && !Array.isArray(value), "the agent read returns an object");
    invariant((value as aether.Agent).id === id, "the agent read returns the requested agent");
    app.store.updateAether(conn, (state) => state.agents[id] ? state : { ...state, agents: { ...state.agents, [id]: value as aether.Agent } });
  }, [app, cached, conn, id, read?.value]);
  if (!info) {
    return <EmptyState title="Not connected">No connection {conn} is open. <a className="ui-link" href="#/fleet">Back to the fleet</a></EmptyState>;
  }
  if (!cached && info.state !== "ready") {
    return <section className="ui-page agent" aria-busy="true" aria-label="Loading the agent"><Skeleton height="3rem" /><Skeleton height="8rem" /></section>;
  }
  if (!cached && read?.error) {
    return <ErrorState title="Agent unavailable" message={`This agent may no longer be available. ${read.error}`} onRetry={() => setRefresh((n) => n + 1)} />;
  }
  const value = cached ?? read?.value;
  if (value == null && read && read.issued > 0) {
    return <ErrorState title="Agent unavailable" message="The daemon returned no agent." onRetry={() => setRefresh((n) => n + 1)} />;
  }
  if (value == null) return <section className="ui-page agent" aria-busy="true" aria-label="Loading the agent"><Skeleton height="3rem" /><Skeleton height="8rem" /></section>;
  invariant(typeof value === "object" && !Array.isArray(value), "the agent read returns an object");
  invariant((value as aether.Agent).id === id, "the agent read returns the requested agent");
  const agent = value as aether.Agent;
  return (
    <section className="ui-page agent" aria-label={`Agent ${agent.name}`}>
      <AgentHeader conn={conn} agent={agent} />
      <AgentFacts agent={agent} />
      <Chat key={`${conn}/${id}`} conn={conn} agent={agent} />
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
