// The Aether tiles: agents by state, what needs you, today's spend and the newest events.

import { routeHref } from "@aether-ui/core";
import { Donut, Sparkline, StatTile } from "./charts.ts";
import type { HomeView } from "./derive.ts";
import { invariant } from "./invariant.ts";
import { formatTokens, formatUsd, relativeTime, spendSlices, SPARK_HOURS } from "./model.ts";
import { nextStep } from "./steps.ts";
import { hintOf, LoadingTile, NextStep, plural, presenceValue } from "./tile-parts.tsx";

export function AgentsTile({ view }: { readonly view: HomeView }) {
  const { facts, agents } = view;
  if (facts.aether === "connecting") return <LoadingTile label="Agents" />;
  const step = nextStep("agents", facts);
  const value = facts.aether === "ready" ? String(agents.running) : presenceValue(facts.aether);
  const hint = facts.aether !== "ready" ? "Aether runs the agents"
    : agents.total === 0 ? "No agents yet"
      : `running · ${hintOf([[agents.waiting, `${agents.waiting} waiting`], [agents.attention, `${agents.attention} need attention`]], `${plural(agents.total, "agent")} in all`)}`;
  return (
    <StatTile label="Agents" value={value} hint={hint} tone={agents.attention > 0 ? "warning" : "accent"} href="#/fleet">
      {step ? <NextStep step={step} /> : <Sparkline label={`Events per hour, last ${SPARK_HOURS} hours`} points={view.buckets} />}
    </StatTile>
  );
}

const ARCHON_QUEUE = "#/archon/queue";

/** The Inbox shows only Aether's needs; Archon's questions live on its Queue, so the tile points there when they are all that waits. */
export function NeedsTile({ view }: { readonly view: HomeView }) {
  const { facts, needs, questions } = view;
  if (facts.aether === "connecting" && facts.archon !== "ready") return <LoadingTile label="Needs you" />;
  const step = nextStep("needs", facts);
  const hint = hintOf([
    [needs.questions, plural(needs.questions, "question")], [needs.approvals, plural(needs.approvals, "approval")],
    [needs.stopped, plural(needs.stopped, "stopped agent")], [needs.archon, plural(needs.archon, "Archon question")],
  ], "Nothing waits on you");
  const onlyArchon = needs.archon > 0 && needs.archon === needs.total;
  const archonStep = needs.archon > 0 && !onlyArchon ? { label: `${plural(needs.archon, "Archon question")} on the Queue`, href: ARCHON_QUEUE } : null;
  return (
    <StatTile label="Needs you" value={step ? presenceValue(facts.aether) : String(needs.total)}
      hint={step ? "Questions, approvals and stopped agents" : hint} tone={needs.total > 0 ? "warning" : "success"} href={onlyArchon ? ARCHON_QUEUE : "#/inbox"}>
      {step ? <NextStep step={step} /> : null}
      {archonStep ? <NextStep step={archonStep} /> : null}
      {questions.state === "problem" ? <p className="home-problem">Archon's questions could not be read: {questions.problem}</p> : null}
    </StatTile>
  );
}

export function SpendTile({ view }: { readonly view: HomeView }) {
  const { facts, spend } = view;
  if (facts.aether === "connecting" || spend.state === "loading") return <LoadingTile label="Spend today" />;
  const step = nextStep("spend", facts);
  if (step && facts.aether !== "ready") {
    const value = presenceValue(facts.aether);
    return <StatTile label="Spend today" value={value} hint="Nothing spent yet" href="#/activity"><NextStep step={step} /></StatTile>;
  }
  if (spend.state === "problem") {
    return <StatTile label="Spend today" value="Unknown" hint="Today's spend could not be read" tone="danger" href="#/activity"><p className="home-problem">{spend.problem}</p><LifetimeUsage lifetime={view.lifetime} /></StatTile>;
  }
  const { usd, tokens, estimatedUsd, listPrice, complete, byModel, source } = spend.value;
  const slices = spendSlices(byModel);
  const amount = !complete && usd === 0 ? "Unknown" : `${formatUsd(usd)}${complete ? "" : "+"}`;
  const coverage = complete ? (source === "breakdown" && (tokens > 0 || usd > 0) ? " · partial if subscription turns lack per-message usage" : "")
    : " · partial: subscription turns may lack per-message usage";
  const hint = `${formatTokens(tokens)} recorded tokens including cache${source === "agents" ? " · lifetime cost of agents active today" : ""}${listPrice ? " · includes list-price estimates" : ""}${coverage}`;
  return (
    <StatTile label={source === "agents" ? "Lifetime spend of active agents" : "Spend today"} value={amount} hint={hint} href="#/activity">
      <LifetimeUsage lifetime={view.lifetime} />
      {estimatedUsd > 0 ? <p className="home-quiet">{formatUsd(estimatedUsd)} at API list price for subscription use; not a per-request charge</p> : null}
      {slices.length > 0 ? <Donut label="Spend by model" slices={slices} center={amount} /> : tokens === 0 && estimatedUsd === 0 && complete ? <p className="home-quiet">Nothing spent yet today</p> : null}
      {step && tokens === 0 && usd === 0 && estimatedUsd === 0 ? <NextStep step={step} /> : null}
    </StatTile>
  );
}

function LifetimeUsage({ lifetime }: { readonly lifetime: HomeView["lifetime"] }) {
  if (lifetime.state === "loading") return <p className="home-quiet">Lifetime status: loading</p>;
  if (lifetime.state === "problem") return <p className="home-quiet">Lifetime status unavailable: {lifetime.problem}</p>;
  const u = lifetime.value;
  const cost = !u.complete && u.usd === 0 ? "cost not reported" : `$${u.usd.toFixed(4)}${u.complete ? "" : "+"}`;
  const estimate = u.estimated === null ? "" : ` · $${u.estimated.toFixed(4)} subscription API-equivalent estimate`;
  return <p className="home-quiet">Lifetime status{u.stale ? " (stale)" : ""}: {u.input.toLocaleString("en-US")} in · {u.cacheWrite.toLocaleString("en-US")} cache-write · {u.cacheRead.toLocaleString("en-US")} cached · {u.output.toLocaleString("en-US")} out · {cost}{u.listPrice ? " (includes list-price estimates)" : ""}{estimate}</p>;
}

export function ActivityTile({ view }: { readonly view: HomeView }) {
  const { facts } = view;
  if (facts.aether === "connecting") return <LoadingTile label="Recent activity" />;
  const step = nextStep("activity", facts);
  const value = facts.aether === "ready" ? String(view.lastHour) : presenceValue(facts.aether);
  return (
    <StatTile label="Recent activity" value={value} hint={facts.aether === "ready" ? "events in the last hour" : "What the agents did, newest first"} href="#/activity">
      {step ? <NextStep step={step} /> : <ActivityList view={view} />}
    </StatTile>
  );
}

function ActivityList({ view }: { readonly view: HomeView }) {
  invariant(view.activity.length > 0, "the list shows at least one event");
  return (
    <ol className="home-list" aria-label="Newest events">
      {view.activity.map((row) => (
        <li key={row.key} className="home-row">
          {row.agent === null ? <span className="home-row-name">{row.name}</span>
            : <a className="ui-link home-row-name" href={routeHref("#/agents/:conn/:agent", { conn: row.conn, agent: row.agent })}>{row.name}</a>}
          <span className="home-row-text">{row.label}</span>
          <time className="home-row-time" dateTime={row.at}>{relativeTime(row.at, view.now)}</time>
        </li>
      ))}
    </ol>
  );
}
