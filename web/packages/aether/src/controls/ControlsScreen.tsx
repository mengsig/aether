// `#/controls/:conn/:agent`: every control the old PWA offered for one agent, grouped in tabs. The
// agent comes from core's store; each control is disabled, with its reason, where this device may not act.

import { useDraft, routeHref, selectAgent, selectConnection, useSelector } from "@aether-ui/core";
import type { aether, ConnId, RouteProps } from "@aether-ui/protocol";
import { Badge, EmptyState, Skeleton, StatusPill, Tabs, TextArea, Switch } from "@aether-ui/ui";
import { useMemo, useState } from "react";
import { invariant } from "../fleet/invariant.ts";
import { agentStatus } from "../fleet/rows.ts";
import { ApprovalsPanel } from "./approvals.tsx";
import { DraftError, sendCommand } from "./commands.ts";
import { ConfigurePanel } from "./configure.tsx";
import { Action, RunNote, Section, StandingNotice, useRun } from "./kit.tsx";
import { LifecyclePanel } from "./lifecycle.tsx";
import { RecordsPanel } from "./records.tsx";
import { SchedulesPanel } from "./schedules.tsx";

function SteerPanel({ conn, agent }: { readonly conn: ConnId; readonly agent: aether.Agent }) {
  const run = useRun(conn);
  const draft = useDraft(`controls/${agent.id}/message`, { text: "", subtree: false });
  const { text, subtree } = draft.value;
  const [problem, setProblem] = useState<string | null>(null);
  const send = async () => {
    try {
      const command = sendCommand(agent.id, text, subtree);
      setProblem(null);
      if (await run.send(command, "Sent")) draft.clear();
    } catch (error) {
      if (!(error instanceof DraftError)) throw error;
      setProblem(error.message);
    }
  };
  return (
    <Section title="Steer" hint="A message reaches the agent at its next turn; with its delegates, each of them gets it too.">
      <TextArea label="Message" rows={3} value={text} onChange={event => draft.update({ text: event.target.value })} />
      <div className="b3-row">
        <Switch label="Also send to the agents it delegated to" checked={subtree} onChange={subtree => draft.update({ subtree })} />
        <Action conn={conn} command={{ cmd: "send_message", to: agent.id }} label="Send" blocked={text.trim() ? null : "Write a message"} variant="primary" busy={run.status.state === "busy"} onRun={send} />
      </div>
      {problem ? <p className="b3-note" role="alert" data-state="failed">{problem}</p> : null}
      <RunNote status={run.status} />
    </Section>
  );
}

function ControlsHeader({ conn, agent }: { readonly conn: ConnId; readonly agent: aether.Agent }) {
  const status = agentStatus(agent);
  return (
    <header className="ui-page-head b3-screen-head">
      <div className="b3-row">
        <h2 className="ui-page-title b3-screen-title">{agent.name}</h2>
        <StatusPill status={status.status} label={status.label} />
        {agent.paused ? <Badge tone="warning">Paused</Badge> : null}
        {agent.archived ? <Badge>Archived</Badge> : null}
      </div>
      <p className="ui-page-lede b3-screen-sub">
        Controls · <code className="b3-code">{agent.id}</code> · <a className="ui-link" href={routeHref("#/agents/:conn/:agent", { conn, agent: agent.id })}>Open the conversation</a>
      </p>
    </header>
  );
}

export function ControlsScreen({ conn, id }: { readonly conn: ConnId; readonly id: string }) {
  invariant(conn !== "" && id !== "", "the controls route names a connection and an agent");
  const info = useSelector(useMemo(() => selectConnection(conn), [conn]));
  const agent = useSelector(useMemo(() => selectAgent(conn, id), [conn, id]));
  const [tab, setTab] = useState("lifecycle");
  if (!info) return <EmptyState title="Not connected">No connection {conn} is open. <a className="ui-link" href="#/fleet">Back to the fleet</a></EmptyState>;
  if (!agent && info.state !== "ready") {
    return <section className="ui-page b3-screen" aria-busy="true" aria-label="Loading the agent"><Skeleton height="3rem" /><Skeleton height="10rem" /></section>;
  }
  if (!agent) return <EmptyState title="No such agent">{info.label} has no agent {id}. <a className="ui-link" href="#/fleet">Back to the fleet</a></EmptyState>;
  invariant(agent.id === id, "the store returns the agent the route names");
  const tabs = [
    { id: "lifecycle", label: "Lifecycle", panel: <div className="b3-panel"><LifecyclePanel conn={conn} agent={agent} /></div> },
    { id: "steer", label: "Steer", panel: <div className="b3-panel"><SteerPanel conn={conn} agent={agent} /></div> },
    { id: "configure", label: "Model & limits", panel: <div className="b3-panel"><ConfigurePanel conn={conn} agent={agent} /></div> },
    { id: "approvals", label: "Approvals", panel: <div className="b3-panel"><ApprovalsPanel conn={conn} agent={agent} /></div> },
    { id: "schedules", label: "Schedules", panel: <div className="b3-panel"><SchedulesPanel conn={conn} agent={agent} /></div> },
    { id: "records", label: "Records", panel: <div className="b3-panel"><RecordsPanel conn={conn} agent={agent} /></div> },
  ];
  return (
    <section className="ui-page b3-screen" aria-label={`Controls for ${agent.name}`}>
      <ControlsHeader conn={conn} agent={agent} />
      <StandingNotice conn={conn} />
      <Tabs label="Agent controls" tabs={tabs} selected={tab} onSelect={setTab} />
    </section>
  );
}

/** The route's screen: `:conn` and `:agent` come from the hash (CONTRACTS.md §2.3). */
export default function ControlsRoute({ params }: RouteProps) {
  return <ControlsScreen conn={params.conn ?? ""} id={params.agent ?? ""} />;
}
