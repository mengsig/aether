// `#/agents/:conn/:agent`: one agent's identity and state in a compact header, its conversation filling
// the rest, and its facts in a Details panel. Everything shown is derived from core's store.

import { routeHref, selectAgent, selectConnection, useReread, useSelector } from "@aether-ui/core";
import type { aether, ConnId, RouteProps } from "@aether-ui/protocol";
import { Badge, Button, EmptyState, EntityActionMenu, Skeleton, StatusPill } from "@aether-ui/ui";
import { useId, useMemo, useState } from "react";
import { Conversation } from "../conversation/Conversation.tsx";
import { agentStatus } from "../fleet/rows.ts";
import { roleLabel } from "../fleet/roles.ts";
import { invariant } from "../fleet/invariant.ts";
import "./agent.css";
import { agentFacts, currentTask, headlineFacts, statusDetail, taskFact, type Fact } from "./facts.ts";

function AgentLink({ conn, id, children }: { readonly conn: ConnId; readonly id: string; readonly children: string }) {
  return <a className="ui-link" href={routeHref("#/agents/:conn/:agent", { conn, agent: id })}>{children}</a>;
}

interface HeaderProps {
  readonly conn: ConnId;
  readonly agent: aether.Agent;
  readonly facts: readonly Fact[];
  readonly details: { readonly id: string; readonly open: boolean; readonly toggle: () => void };
}

function AgentHeader({ conn, agent, facts, details }: HeaderProps) {
  const status = agentStatus(agent);
  const role = roleLabel(agent.role);
  const parent = useSelector(useMemo(() => selectAgent(conn, agent.parent ?? ""), [conn, agent.parent]));
  const detail = statusDetail(agent.status);
  return (
    <header className="agent-head">
      <div className="agent-title">
        <h2 className="ui-page-title agent-name">{agent.name}</h2>
        <StatusPill status={status.status} label={status.label} />
        {agent.paused ? <Badge tone="warning">Paused</Badge> : null}
        {agent.archived ? <Badge>Archived</Badge> : null}
        <p className="agent-headline">
          {headlineFacts(facts).map((fact) => <span key={fact.label} title={fact.label}>{fact.value}</span>)}
        </p>
        <Button variant="quiet" size="sm" className="agent-details-toggle" aria-expanded={details.open} aria-controls={details.id} onClick={details.toggle}>
          Details<span className="conv-chevron" aria-hidden="true" />
        </Button>
        <EntityActionMenu entity={{ kind: "agent", conn, id: agent.id }} />
      </div>
      <p className="agent-sub">
        <span title={agent.role}>{role.kind === "agent" ? role.text || "No role" : <Badge tone="accent">{role.text}</Badge>}</span>
        <code className="agent-id">{agent.id}</code>
        {agent.parent ? <span>Delegated by <AgentLink conn={conn} id={agent.parent}>{parent?.name ?? agent.parent}</AgentLink></span> : null}
      </p>
      {detail ? <p className="agent-detail" data-state={status.state}>{detail}</p> : null}
    </header>
  );
}

function AgentFacts({ id, open, facts }: { readonly id: string; readonly open: boolean; readonly facts: readonly Fact[] }) {
  return (
    <aside className="agent-aside" id={id} data-open={open} aria-label="Details">
      <dl className="agent-facts">
        {facts.map((fact) => (
          <div key={fact.label} className="agent-fact" data-testid={`agent-fact-${fact.label.toLowerCase().replaceAll(" ", "-")}`}>
            <dt>{fact.label}</dt>
            <dd title={fact.hint}>{fact.value}{fact.hint ? <span className="agent-hint">{fact.hint}</span> : null}</dd>
          </div>
        ))}
      </dl>
    </aside>
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

function AgentView({ conn, agent }: { readonly conn: ConnId; readonly agent: aether.Agent }) {
  const facts = useFacts(conn, agent);
  const [open, setOpen] = useState(false);
  const id = useId();
  const details = useMemo(() => ({ id, open, toggle: () => setOpen((was) => !was) }), [id, open]);
  return (
    <section className="ui-page agent" aria-label={`Agent ${agent.name}`}>
      <AgentHeader conn={conn} agent={agent} facts={facts} details={details} />
      <div className="agent-body" data-details={open}>
        <Conversation key={`${conn}/${agent.id}`} conn={conn} agent={agent.id} />
        <AgentFacts id={id} open={open} facts={facts} />
      </div>
    </section>
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
  invariant(agent.id === id, `the store returned agent ${agent.id} for ${id}`);
  return <AgentView conn={conn} agent={agent} />;
}

/** The route's screen: `:conn` and `:agent` come from the hash (CONTRACTS.md §2.3). */
export default function AgentRoute({ params }: RouteProps) {
  const conn = params.conn ?? "";
  const agent = params.agent ?? "";
  if (conn === "" || agent === "") return <EmptyState title="No agent named">The address names no connection or agent.</EmptyState>;
  return <AgentScreen conn={conn} id={agent} />;
}
