// `#/github/:host/:owner/:repo/:number`: one issue or pull request as Archon's GitHub cache last read it,
// joined with the items linked to it, their runs and outcomes, and the Aether agents that worked on them.

import { currentHash } from "@aether-ui/core";
import type { archon, ConnectionInfo, RouteProps } from "@aether-ui/protocol";
import { Badge, EmptyState, EntityActionMenu, ErrorState, Skeleton } from "@aether-ui/ui";
import { useCallback, useMemo, useState } from "react";
import { readStatus, STALE_CHECK_MS, useAether, useArchonConnection, useChangeKey, useGithubReads, useItems, useLinkedItems, useNow } from "./data.ts";
import { githubFact } from "./facts.ts";
import { ImportAction } from "./ImportAction.tsx";
import { isStale, joinPage, type PageView } from "./join.ts";
import { entityId, githubUrl, invariant, parsePage, type PageRef } from "./ref.ts";
import { Agents, FactCard, ItemCard, Section } from "./Sections.tsx";
import "./github.css";

function Loading({ label }: { readonly label: string }) {
  return (
    <div className="ui-page gh-page" aria-busy="true" aria-label={label}>
      <Skeleton height="2rem" width="55%" /><Skeleton height="5rem" /><Skeleton height="10rem" />
    </div>
  );
}

function Notices({ info, error, stale, fetchedAt }: { readonly info: ConnectionInfo; readonly error: string | null; readonly stale: boolean; readonly fetchedAt: string | null }) {
  const notes: { tone: "warning" | "neutral"; text: string }[] = [];
  if (info.standing === "observer") notes.push({ tone: "neutral", text: "This device observes Archon: it can read this page, but not import or change anything." });
  if (stale && fetchedAt) notes.push({ tone: "warning", text: `Archon last read this from GitHub at ${new Date(fetchedAt).toLocaleString()}; GitHub may have moved on since.` });
  if (error) notes.push({ tone: "warning", text: `Showing the last read: rereading failed (${error}).` });
  if (notes.length === 0) return null;
  return <div className="gh-notices">{notes.map((note) => <p key={note.text} className="gh-note" data-tone={note.tone} role="note">{note.text}</p>)}</div>;
}

interface Body { readonly info: ConnectionInfo; readonly page: PageRef; readonly view: PageView; readonly error: string | null; readonly reload: () => void }

function Header({ page, view }: Pick<Body, "page" | "view">) {
  const object = view.object;
  const kindLabel = view.kind === "pull_request" ? "Pull request" : view.kind === "issue" ? "Issue" : "GitHub";
  const meta = [kindLabel, `${page.repo}#${page.number}`, object?.author ? `by ${object.author}` : null].filter(Boolean).join(" · ");
  return (
    <header className="gh-header">
      <p className="ui-eyebrow gh-eyebrow">{meta}</p>
      <h1 id="gh-title" className="ui-page-title gh-title">{object?.title ?? `${page.repo}#${page.number}`}</h1>
      {object && object.labels.length > 0 ? <div className="gh-labels">{object.labels.map((label) => <Badge key={label}>{label}</Badge>)}</div> : null}
    </header>
  );
}

function Actions({ info, page, view, reload }: Omit<Body, "error">) {
  const entity = { kind: view.kind ?? "issue", conn: info.id, id: entityId(page) } as const;
  const importedLink = view.items.find((item) => item.relations.some((relation) => relation === "imported" || relation === "origin"));
  const imported = importedLink ? { id: importedLink.id, title: importedLink.title } : null;
  const startOpen = useMemo(() => currentHash().endsWith("?import"), []);
  return (
    <div className="gh-actions" role="group" aria-label="Actions">
      {view.kind !== "pull_request" ? (
        <ImportAction conn={info.id} page={page} served={info.features.includes("github_import")} object={view.object}
          imported={imported} startOpen={startOpen} onDone={reload} />
      ) : null}
      <a className="ui-link gh-external" href={githubUrl(page, view.kind)} target="_blank" rel="noopener noreferrer">Open on GitHub</a>
      {view.kind !== null ? <EntityActionMenu entity={entity} label="More actions" /> : null}
    </div>
  );
}

function PageBody({ info, page, view, error, reload }: Body) {
  const now = useNow(STALE_CHECK_MS);
  const stale = view.object !== null && isStale(view.object, now);
  const facts = [githubFact(view.kind, view.object, view.recorded)];
  invariant(facts.every((fact) => fact.status === null || !["succeeded", "failed", "running"].includes(fact.status)), "a GitHub state never takes an item's colour");
  return (
    <article className="ui-page gh-page" aria-labelledby="gh-title">
      <Header page={page} view={view} />
      <div className="gh-facts" role="group" aria-label="GitHub state">{facts.map((fact) => <FactCard key={fact.label} fact={fact} />)}</div>
      <Notices info={info} error={error} stale={stale} fetchedAt={view.object?.fetched_at ?? null} />
      <Actions info={info} page={page} view={view} reload={reload} />
      <div className="gh-grid">
        <Section title="Archon work">
          {view.items.length === 0 ? <p className="gh-muted">No Archon item is linked to this yet.</p>
            : <ul className="gh-list gh-items">{view.items.map((item) => <ItemCard key={item.id} view={item} />)}</ul>}
        </Section>
        <Section title="Agents"><AgentsFor view={view} /></Section>
      </div>
    </article>
  );
}

function AgentsFor({ view }: { readonly view: PageView }) {
  return <Agents items={view.items} aether={useAether()} />;
}

function GithubPage({ info, page }: { readonly info: ConnectionInfo; readonly page: PageRef }) {
  const [nonce, setNonce] = useState(0);
  const changes = useChangeKey(info.id);
  const reads = useGithubReads(info, page, `${String(changes)}|${nonce}`);
  const ids = useMemo(() => [...new Set(reads.links.map((link) => link.item))], [reads.links]);
  const items = useItems(info.id);
  const linked = useLinkedItems(info.id, info.state === "ready", ids);
  const cached = (reads.cache?.value as { objects?: archon.GithubObject[] } | null | undefined)?.objects ?? null;
  const view = useMemo(() => joinPage({ page, cached, links: reads.links, items, ...linked }), [page, cached, reads.links, items, linked]);
  const reload = useCallback(() => setNonce((n) => n + 1), []);
  const status = readStatus([reads.cache, ...reads.linkReads]);
  if (status.loading) return <Loading label="Loading the GitHub page" />;
  if (!status.answered && status.error) return <ErrorState title="Could not read this from Archon" message={status.error} onRetry={reload} />;
  if (view.object === null && view.items.length === 0) return <Unknown info={info} page={page} view={view} reload={reload} />;
  return <PageBody info={info} page={page} view={view} error={status.error} reload={reload} />;
}

function Unknown({ info, page, view, reload }: Omit<Body, "error">) {
  return (
    <div className="ui-page gh-page">
      <EmptyState title={`Archon knows nothing of ${page.repo}#${page.number} yet`}>
        Archon's GitHub cache has not read it and no item is linked to it.
      </EmptyState>
      <Notices info={info} error={null} stale={false} fetchedAt={null} />
      <Actions info={info} page={page} view={view} reload={reload} />
    </div>
  );
}

function Connected({ info, page }: { readonly info: ConnectionInfo; readonly page: PageRef }) {
  if (info.state !== "ready") {
    if (info.state === "closed") return <EmptyState title="Archon is not connected">Connect to archond to see this issue or pull request.</EmptyState>;
    return <Loading label="Connecting to Archon" />;
  }
  const served = info.features.includes("github_links") || info.features.includes("github_import");
  if (!served) return <EmptyState title="This archond does not track GitHub">It does not serve GitHub links or its GitHub cache; update archond to see issue and pull request pages.</EmptyState>;
  return <GithubPage info={info} page={page} />;
}

export default function GithubRoute({ params }: RouteProps) {
  const { host = "", owner = "", repo = "", number = "" } = params;
  const page = useMemo(() => parsePage({ host, owner, repo, number }), [host, owner, repo, number]);
  const info = useArchonConnection();
  if (page === null) return <EmptyState title="Not a GitHub issue or pull request">This link does not name a repository and number.</EmptyState>;
  if (!info) return <EmptyState title="Archon is not connected">GitHub pages read Archon's links and cache; connect to archond to see them.</EmptyState>;
  return <Connected info={info} page={page} />;
}
