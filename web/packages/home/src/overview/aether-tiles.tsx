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

export function NeedsTile({ view }: { readonly view: HomeView }) {
  const { facts, needs, questions } = view;
  if (facts.aether === "connecting" && facts.archon !== "ready") return <LoadingTile label="Needs you" />;
  const step = nextStep("needs", facts);
  const hint = hintOf([
    [needs.questions, plural(needs.questions, "question")], [needs.approvals, plural(needs.approvals, "approval")],
    [needs.stopped, plural(needs.stopped, "stopped agent")], [needs.archon, plural(needs.archon, "Archon question")],
  ], "Nothing waits on you");
  return (
    <StatTile label="Needs you" value={step ? presenceValue(facts.aether) : String(needs.total)}
      hint={step ? "Questions, approvals and stopped agents" : hint} tone={needs.total > 0 ? "warning" : "success"} href="#/inbox">
      {step ? <NextStep step={step} /> : null}
      {questions.state === "problem" ? <p className="home-problem">Archon's questions could not be read: {questions.problem}</p> : null}
    </StatTile>
  );
}

export function SpendTile({ view }: { readonly view: HomeView }) {
  const { facts, spend } = view;
  if (facts.aether === "connecting" || spend.state === "loading") return <LoadingTile label="Spend today" />;
  const step = nextStep("spend", facts);
  if (step) {
    const value = facts.aether === "ready" ? formatUsd(0) : presenceValue(facts.aether);
    return <StatTile label="Spend today" value={value} hint="Nothing spent yet" href="#/activity"><NextStep step={step} /></StatTile>;
  }
  if (spend.state === "problem") {
    return <StatTile label="Spend today" value="Unknown" hint="Today's spend could not be read" tone="danger" href="#/activity"><p className="home-problem">{spend.problem}</p></StatTile>;
  }
  const { usd, tokens, complete, byModel, source } = spend.value;
  const slices = spendSlices(byModel);
  const amount = `${formatUsd(usd)}${complete ? "" : "+"}`;
  const hint = `${formatTokens(tokens)} tokens${source === "agents" ? " · lifetime cost of agents active today" : ""}${complete ? "" : " · leaves out some work"}`;
  return (
    <StatTile label="Spend today" value={amount} hint={hint} href="#/activity">
      {slices.length === 0 ? <p className="home-quiet">Nothing spent yet today</p> : <Donut label="Spend by model" slices={slices} center={amount} />}
    </StatTile>
  );
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
