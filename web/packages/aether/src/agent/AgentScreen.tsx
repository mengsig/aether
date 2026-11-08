// `#/agents/:conn/:agent`: one agent's identity, state, model, cost and budget, above its
// chat. Everything shown is derived from core's store; this screen keeps no daemon data.

import { routeHref, selectAgent, selectAttention, selectConnection, useApp, useReread, useSelector } from "@aether-ui/core";
import type { aether, ConnId, RouteProps } from "@aether-ui/protocol";
import { Badge, EmptyState, EntityActionMenu, ErrorState, Skeleton, StatusPill } from "@aether-ui/ui";
import { useEffect, useMemo, useState } from "react";
import { Chat } from "../conversation/Chat.tsx";
import { agentStatus } from "../fleet/rows.ts";
import { roleLabel } from "../fleet/roles.ts";
import { invariant } from "../fleet/invariant.ts";
import "./agent.css";
import { agentFacts, currentTask, statusDetail, taskFact, type Fact } from "./facts.ts";

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

/** The agent's facts with its current task first; the task list is re-read whenever the held task changes. */
function useFacts(conn: ConnId, agent: aether.Agent): Fact[] {
  const tasks = useReread(conn, `agent:${agent.id}:tasks`, { cmd: "list_tasks", query: { owner: agent.id } }, ["tasks"], agent.current_task ?? null);
  const listed = tasks?.value;
  return useMemo(() => {
    const task = Array.isArray(listed) ? currentTask(agent, listed as aether.Task[]) : null;
    return task === null ? agentFacts(agent) : [taskFact(task), ...agentFacts(agent)];
  }, [agent, listed]);
}

function AgentFacts({ conn, agent }: { readonly conn: ConnId; readonly agent: aether.Agent }) {
  const facts = useFacts(conn, agent);
  return (
    <dl className="ui-card agent-facts">
      {facts.map((fact) => (
        <div key={fact.label} className="agent-fact" data-testid={`agent-fact-${fact.label.toLowerCase()}`}>
          <dt>{fact.label}</dt>
          <dd title={fact.hint}>{fact.value}{fact.hint ? <span className="agent-hint">{fact.hint}</span> : null}</dd>
        </div>
      ))}
    </dl>
  );
}

function AgentLoading() {
  return <section className="ui-page agent" aria-busy="true" aria-label="Loading the agent"><Skeleton height="3rem" /><Skeleton height="8rem" /></section>;
}

function MissingAgent({ conn, id }: { readonly conn: ConnId; readonly id: string }) {
  const app = useApp();
  const [refresh, setRefresh] = useState(0);
  const [failure, setFailure] = useState<{ attempt: number; message: string }>();
  useEffect(() => {
    let current = true;
    app.send(conn, { cmd: "get_agent", id }).then((reply) => {
      invariant(reply.reply === "agent" && reply.data !== null && typeof reply.data === "object" && !Array.isArray(reply.data), "get_agent returns an agent");
      const agent = reply.data as aether.Agent;
      invariant(agent.id === id, "get_agent returns the requested agent");
      if (current) app.store.updateAether(conn, (state) => state.agents[id] ? state : { ...state, agents: { ...state.agents, [id]: agent } });
    }).catch((error: unknown) => {
      if (current) setFailure({ attempt: refresh, message: error instanceof Error ? error.message : String(error) });
    });
    return () => { current = false; };
  }, [app, conn, id, refresh]);
  if (failure?.attempt === refresh) {
    return <ErrorState title="Agent unavailable" message={`This agent may no longer be available. ${failure.message}`} onRetry={() => setRefresh((n) => n + 1)} />;
  }
  return <AgentLoading />;
}

export function AgentScreen({ conn, id }: { readonly conn: ConnId; readonly id: string }) {
  invariant(conn !== "" && id !== "", "the agent route names a connection and an agent");
  const info = useSelector(useMemo(() => selectConnection(conn), [conn]));
  const agent = useSelector(useMemo(() => selectAgent(conn, id), [conn, id]));
  if (!info) {
    return <EmptyState title="Not connected">No connection {conn} is open. <a className="ui-link" href="#/fleet">Back to the fleet</a></EmptyState>;
  }
  if (!agent && info.state !== "ready") return <AgentLoading />;
  if (!agent) return <MissingAgent key={`${conn}/${id}`} conn={conn} id={id} />;
  return (
    <section className="ui-page agent" aria-label={`Agent ${agent.name}`}>
      <AgentHeader conn={conn} agent={agent} />
      <AgentFacts conn={conn} agent={agent} />
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
