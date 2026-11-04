// `#/archon/items/:item`: one item's execution, pull request and issue states side by side, what holds it,
// the operator's actions, its runs, stages, gates, questions, links, artifacts and reports.

import { routeHref, selectItem, selectItems, useSelector } from "@aether-ui/core";
import type { archon, ConnectionInfo, RouteProps } from "@aether-ui/protocol";
import { EmptyState, ErrorState, Skeleton, StatusPill } from "@aether-ui/ui";
import { useMemo } from "react";
import { useActivity, useArchonAccess, useArchonConnection, useRead, useSettled, type ArchonAccess } from "../queue/archon.ts";
import { invariant, isStateKind, type StateKind } from "../queue/model.ts";
import { NotConnected, useNow } from "../queue/QueueScreen.tsx";
import { QuestionList } from "../queue/Questions.tsx";
import { Artifacts } from "./Artifacts.tsx";
import { useItemDetail } from "./detail.ts";
import { executionFact, issueFact, issueOf, objectFor, outcome, prFact, prOf, type Fact } from "./facts.ts";
import { ItemActions } from "./ItemActions.tsx";
import { Details, Gates, Links, Reports, Runs, Section, Stages } from "./ItemSections.tsx";
import "./item.css";

interface LinksReply { readonly links: archon.GithubLink[]; readonly objects: archon.GithubObject[] }
const NO_LINKS: LinksReply = { links: [], objects: [] };

function FactCard({ fact }: { readonly fact: Fact }) {
  return (
    <div className="item-fact" aria-label={`${fact.label}: ${fact.text}`}>
      <span className="item-fact-label">{fact.label}</span>
      {fact.status ? <StatusPill status={fact.status} label={fact.text} /> : <span className="item-fact-none">{fact.text}</span>}
      {fact.detail ? <span className="item-muted">{fact.detail}</span> : null}
    </div>
  );
}

function Facts({ kind, detail, summary, links, stage }: {
  readonly kind: StateKind; readonly detail: archon.ItemDetail; readonly summary: archon.ItemSummary | null;
  readonly links: LinksReply; readonly stage: string | null;
}) {
  const issue = issueOf(summary, links.links);
  const pr = prOf(summary, links.links);
  const facts = [executionFact(kind, detail, stage), prFact(pr, objectFor(pr, links.objects)), issueFact(issue, objectFor(issue, links.objects))];
  invariant(new Set(facts.map((fact) => fact.label)).size === 3, "execution, PR and issue are three separate facts");
  return <div className="item-facts" role="group" aria-label="States">{facts.map((fact) => <FactCard key={fact.label} fact={fact} />)}</div>;
}

function Outcome({ detail }: { readonly detail: archon.ItemDetail }) {
  const said = outcome(detail);
  if (!said) return null;
  return (
    <div className="item-callout" data-tone={said.tone} role="note">
      <p className="item-callout-title">{said.title}</p>
      <p className="item-callout-text">{said.text}</p>
    </div>
  );
}

interface PageProps {
  readonly access: ArchonAccess; readonly detail: archon.ItemDetail; readonly kind: StateKind;
  readonly summary: archon.ItemSummary | null; readonly stage: string | null; readonly reload: () => void;
}

function ItemPage({ access, detail, kind, summary, stage, reload }: PageProps) {
  const item = detail.item;
  const now = useNow();
  // The pause and the questions move with any activity; the rest follows each reread of this detail.
  const settled = useSettled(useActivity(access.conn), 1_000);
  const repos = useRead<archon.Repo[]>(access, { cmd: "repo_list" }, ["repos"]);
  const status = useRead<archon.DaemonStatus>(access, { cmd: "status" }, ["status"], settled);
  const questions = useRead<archon.Question[]>(access, { cmd: "list_questions", open_only: false }, ["questions"], settled);
  const reports = useRead<archon.ReportSummary[]>(access, { cmd: "list_reports" }, ["reports"], detail);
  const links = useRead<LinksReply>(access, access.has("github_links") ? { cmd: "item_links", item: item.id } : null, ["item_links"], detail);
  const repoName = item.repo ? repos.data?.find((repo) => repo.id === item.repo)?.name ?? null : null;
  const mine = (questions.data ?? []).filter((question) => question.item === item.id && !question.answer);
  const reported = (reports.data ?? []).filter((report) => report.item === item.id);
  const done = () => { reload(); status.reload(); questions.reload(); };
  const pipeline = summary?.switched_to ?? detail.switched_to ?? item.pipeline;
  return (
    <article className="item" aria-labelledby="item-title">
      <header className="item-header">
        <a className="ui-link item-back" href={routeHref("#/archon/queue", {})}>Queue</a>
        <h1 id="item-title" className="item-title">{item.title}</h1>
        <p className="item-muted">{[repoName ?? item.repo ?? "no repository", pipeline, item.id].join(" · ")}{access.live ? "" : " · polled (protocol 23)"}</p>
        <Facts kind={kind} detail={detail} summary={summary} links={links.data ?? NO_LINKS} stage={stage} />
      </header>
      <Outcome detail={detail} />
      <ItemActions access={access} item={item} kind={kind} repoName={repoName} pause={status.data?.pause ?? null} onDone={done} />
      {mine.length > 0 ? <QuestionList access={access} questions={mine} titles={null} onAnswered={done} /> : null}
      <div className="item-grid">
        <Section title="Details"><Details detail={detail} repoName={repoName} pipeline={pipeline} prUrl={summary?.pr_url ?? null} /></Section>
        <Section title="Brief"><p className="item-brief">{item.brief}</p></Section>
        <Section title="Runs"><Runs item={item.id} runs={detail.runs} now={now} /></Section>
        <Section title="Stages"><Stages detail={detail} now={now} /></Section>
        {detail.gates?.length ? <Section title="Gates"><Gates gates={detail.gates} /></Section> : null}
        <Section title="Links">
          {access.has("github_links") ? <Links links={links.data?.links ?? []} objects={links.data?.objects ?? []} />
            : <p className="item-muted">This archond does not list GitHub links; the PR and issue above come from the queue summary.</p>}
          {links.error ? <p role="alert" className="item-error">{links.error}</p> : null}
        </Section>
        <Section title="Artifacts"><Artifacts access={access} item={item.id} version={detail} /></Section>
        {reported.length > 0 ? <Section title="Reports"><Reports reports={reported} /></Section> : null}
      </div>
    </article>
  );
}

function Item({ info, item }: { readonly info: ConnectionInfo; readonly item: string }) {
  const access = useArchonAccess(info);
  const stored = useSelector(useMemo(() => selectItem(access.conn, item), [access.conn, item]));
  const known = useSelector(useMemo(() => selectItems(access.conn), [access.conn]));
  const read = useItemDetail(access, item);
  const detail = read.detail;
  if (!detail) {
    if (read.error) return <ErrorState title="Could not load the item" message={read.error} onRetry={read.reload} />;
    const missing = access.ready && stored === undefined && Object.keys(known).length > 0;
    if (missing) return <EmptyState title="Item not found">Archon has no item {item}. <a className="ui-link" href={routeHref("#/archon/queue", {})}>Back to the queue</a></EmptyState>;
    return <div className="item" aria-busy="true" aria-label="Loading the item"><Skeleton height="2rem" width="60%" /><Skeleton height="6rem" /><Skeleton height="12rem" /></div>;
  }
  invariant(detail.item.id === item, `the detail of ${item} describes ${detail.item.id}`);
  const live = stored && isStateKind(stored.state) ? stored.state : null;
  const kind = live ?? detail.item.state.state;
  const summary = (stored?.summary ?? null) as archon.ItemSummary | null;
  return <ItemPage access={access} detail={detail} kind={kind} summary={summary} stage={stored?.stage ?? summary?.stage ?? null} reload={read.reload} />;
}

export default function ItemRoute({ params }: RouteProps) {
  const info = useArchonConnection();
  const item = params.item;
  if (!item) return <EmptyState title="No item named">Open an item from the queue.</EmptyState>;
  return info ? <Item info={info} item={item} /> : <NotConnected />;
}
