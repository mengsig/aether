// The parts of a GitHub page: its states, the linked items with their runs and outcomes, the Aether agents
// that worked on them.

import { routeHref } from "@aether-ui/core";
import { Badge, Skeleton, StatusPill } from "@aether-ui/ui";
import type { ReactNode } from "react";
import type { AetherSide } from "./data.ts";
import type { StateFact } from "./facts.ts";
import type { AgentUse, ItemView } from "./join.ts";
import { invariant } from "./ref.ts";

export function FactCard({ fact }: { readonly fact: StateFact }) {
  return (
    <div className="gh-fact" aria-label={`${fact.label}: ${fact.text}`} data-fact={fact.label}>
      <span className="gh-fact-label">{fact.label}</span>
      {fact.status ? <StatusPill status={fact.status} label={fact.text} /> : <span className="gh-fact-none">{fact.text}</span>}
      {fact.detail ? <span className="gh-muted">{fact.detail}</span> : null}
    </div>
  );
}

export function Section({ title, children }: { readonly title: string; readonly children: ReactNode }) {
  return (
    <section className="gh-section" aria-label={title}>
      <h2 className="gh-heading">{title}</h2>
      {children}
    </section>
  );
}

function Runs({ item, runs }: { readonly item: string; readonly runs: ItemView["runs"] }) {
  if (runs === null) return <div aria-busy="true" aria-label="Loading the runs"><Skeleton height="1.25rem" width="70%" /></div>;
  if (runs.length === 0) return <p className="gh-muted">No runs yet.</p>;
  return (
    <ul className="gh-list" aria-label="Runs">
      {runs.map((run) => (
        <li key={run.id} className="gh-run">
          {run.status ? <StatusPill status={run.status} label={run.label} /> : <span>{run.label}</span>}
          <a className="ui-link" href={routeHref("#/archon/items/:item/runs/:run", { item, run: run.id })}>{run.id}</a>
        </li>
      ))}
    </ul>
  );
}

export function ItemCard({ view }: { readonly view: ItemView }) {
  return (
    <li className="gh-item" data-item={view.id}>
      <div className="gh-item-head">
        <a className="ui-link gh-item-title" href={routeHref("#/archon/items/:item", { item: view.id })}>{view.title}</a>
        {view.relations.map((relation) => <Badge key={relation} tone="neutral">{relation}</Badge>)}
      </div>
      <div className="gh-facts" role="group" aria-label={`States of ${view.title}`}>
        <FactCard fact={view.execution} />
        {view.pr ? <FactCard fact={view.pr} /> : null}
      </div>
      {view.outcome ? <p className="gh-outcome">{view.outcome}</p> : null}
      <Runs item={view.id} runs={view.runs} />
    </li>
  );
}

interface AgentRow { readonly use: AgentUse; readonly item: ItemView }

function AgentLine({ row, aether }: { readonly row: AgentRow; readonly aether: AetherSide }) {
  const found = aether.find(row.use.agent);
  const stages = `${row.item.title} · ${row.use.stages.join(", ")}`;
  if (found === null) {
    return <li className="gh-agent"><code>{row.use.agent}</code><span className="gh-muted">not on the connected Aether · {stages}</span></li>;
  }
  return (
    <li className="gh-agent">
      <a className="ui-link" href={routeHref("#/agents/:conn/:agent", { conn: found.conn, agent: found.agent.id })}>{found.agent.name}</a>
      <Badge tone="neutral">{found.agent.status.state}</Badge>
      <span className="gh-muted">{stages}</span>
    </li>
  );
}

/** The Aether agents the linked items' stages ran on; with Archon alone the page says where they would show. */
export function Agents({ items, aether }: { readonly items: readonly ItemView[]; readonly aether: AetherSide }) {
  if (!aether.connected) return <p className="gh-muted">Connect Aether to see the agents that worked on this.</p>;
  const rows: AgentRow[] = items.flatMap((item) => item.agents.map((use) => ({ use, item })));
  invariant(rows.every((row) => row.use.stages.length > 0), "an agent is listed for the stages it ran");
  if (rows.length === 0) return <p className="gh-muted">No Aether agent has worked on this yet.</p>;
  return <ul className="gh-list" aria-label="Agents">{rows.map((row) => <AgentLine key={`${row.item.id} ${row.use.agent}`} row={row} aether={aether} />)}</ul>;
}

