// The Archon and Noesis tiles: the queue with its newest outcomes, the GitHub watcher and learning.

import { routeHref } from "@aether-ui/core";
import { StatusPill } from "@aether-ui/ui";
import { BarChart, Donut, StatTile } from "./charts.ts";
import type { HomeView } from "./derive.ts";
import { invariant } from "./invariant.ts";
import { relativeTime } from "./model.ts";
import { nextStep } from "./steps.ts";
import { hintOf, LoadingTile, NextStep, plural, presenceValue } from "./tile-parts.tsx";

const OUTCOME_LABEL = { succeeded: "Succeeded", failed: "Failed", cancelled: "Cancelled" } as const;

export function QueueTile({ view }: { readonly view: HomeView }) {
  const { facts, queue } = view;
  if (facts.archon === "connecting") return <LoadingTile label="Archon queue" />;
  const step = nextStep("queue", facts);
  const active = queue.queued + queue.running + queue.blocked;
  const value = facts.archon === "ready" ? String(active) : presenceValue(facts.archon);
  const hint = facts.archon !== "ready" ? "Archon queues work for the agents" : queue.total === 0 ? "Nothing queued yet"
    : `active · ${hintOf([[queue.queued, `${queue.queued} queued`], [queue.running, `${queue.running} running`], [queue.blocked, `${queue.blocked} blocked`]], "all finished")}`;
  return (
    <StatTile label="Archon queue" value={value} hint={hint} tone={queue.blocked > 0 ? "warning" : "accent"} href="#/archon/queue">
      {step ? <NextStep step={step} /> : <Outcomes view={view} />}
    </StatTile>
  );
}

function Outcomes({ view }: { readonly view: HomeView }) {
  if (view.outcomes.length === 0) return <p className="home-quiet">No finished items yet</p>;
  return (
    <ol className="home-list" aria-label="Recent outcomes">
      {view.outcomes.map((outcome) => (
        <li key={outcome.id} className="home-row">
          <a className="ui-link home-row-name" href={routeHref("#/archon/items/:item", { item: outcome.id })}>{outcome.title}</a>
          <StatusPill status={outcome.kind} label={OUTCOME_LABEL[outcome.kind]} />
          {outcome.at === "" ? null : <time className="home-row-time" dateTime={outcome.at}>{relativeTime(outcome.at, view.now)}</time>}
        </li>
      ))}
    </ol>
  );
}

export function GithubTile({ view }: { readonly view: HomeView }) {
  const { facts, github } = view;
  if (facts.archon === "connecting" || (facts.archon === "ready" && github.state === "loading")) return <LoadingTile label="GitHub" />;
  const step = nextStep("github", facts);
  if (github.state === "problem") {
    return <StatTile label="GitHub" value="Unknown" hint="The watcher could not be read" tone="danger" href="#/github"><p className="home-problem">{github.problem}</p></StatTile>;
  }
  if (step || github.state !== "ready") {
    const value = facts.archon === "ready" ? "0" : presenceValue(facts.archon);
    return <StatTile label="GitHub" value={value} hint="Watched repositories" href="#/github">{step ? <NextStep step={step} /> : null}</StatTile>;
  }
  const { repos, dispatched, ignored, lastPoll, enabled, errors } = github.value;
  const polled = lastPoll === null ? "not polled yet" : `polled ${relativeTime(lastPoll, view.now)}`;
  const hint = enabled ? `${plural(repos, "watched repository", "watched repositories")} · ${polled}` : "The watcher is off";
  const bars = [{ label: "Dispatched", value: dispatched, tone: "accent" as const }, { label: "Ignored", value: ignored, tone: "neutral" as const }];
  return (
    <StatTile label="GitHub" value={String(repos)} hint={hint} tone={errors > 0 ? "danger" : enabled ? "accent" : "neutral"} href="#/github">
      <BarChart label="Issues and pull requests the watcher saw" bars={bars} />
      {errors > 0 ? <p className="home-problem">{plural(errors, "repository", "repositories")} failed the last poll</p> : null}
    </StatTile>
  );
}

const PHASE: Readonly<Record<string, string>> = { probe: "Finding your starting point", plan: "Planning the path", teach: "Learning", practice: "Practising" };

export function LearnTile({ view }: { readonly view: HomeView }) {
  const { learn, facts } = view;
  if (learn.state === "connecting" || learn.state === "loading") return <LoadingTile label="Learn" />;
  const step = nextStep("learn", facts);
  if (learn.state !== "on") {
    const value = learn.state === "off" ? "Off" : learn.state === "unsupported" ? "Unavailable" : presenceValue(learn.state);
    invariant(step !== null, "Noesis that is not on always has a next step");
    return <StatTile label="Learn" value={value} hint="Noesis teaches from what your agents do" tone="neutral" href="#/learn"><NextStep step={step} /></StatTile>;
  }
  const session = learn.session;
  const hint = session === null ? "No session yet" : `${PHASE[session.phase] ?? session.phase}: ${session.goal}`;
  return (
    <StatTile label="Learn" value="On" hint={hint} tone="accent" href="#/learn">
      {step ? <NextStep step={step} /> : null}
      <Mastery view={view} />
    </StatTile>
  );
}

function Mastery({ view }: { readonly view: HomeView }) {
  const { mastery } = view.learn;
  if (mastery.state === "loading") return <p className="home-quiet" aria-busy="true">Reading what you know…</p>;
  if (mastery.state === "problem") return <p className="home-problem">Mastery could not be read: {mastery.problem}</p>;
  const total = mastery.value.reduce((sum, slice) => sum + slice.value, 0);
  if (total === 0) return <p className="home-quiet">No concepts seen yet</p>;
  return <Donut label="Concepts by mastery" slices={mastery.value} center={plural(total, "concept")} />;
}
