import { connectionDraftKey, routeHref, useDraft, useReread, type ReadEntry } from "@aether-ui/core";
import { GraphCanvas } from "@aether-ui/graph";
import { archon, type ConnectionInfo } from "@aether-ui/protocol";
import { Badge, BarChart, Button, Donut, EmptyState, ErrorState, Picker, Skeleton, Sparkline, StatTile, StatusPill } from "@aether-ui/ui";
import { useMemo, useState } from "react";
import { useAether, useArchonConnection, useChangeKey, useItems, useNow, STALE_CHECK_MS, OBJECTS_LIMIT } from "../issue/data.ts";
import { invariant } from "../issue/ref.ts";
import { issueFact, pullRequestFact } from "../issue/facts.ts";
import { useOverviewRows } from "./data.ts";
import { DEFAULT_FILTERS, filterRows, itemsPerPr, objectHref, objectId, relationshipGraph, relationshipLayout, weeklyBuckets, type ObjectRow, type OverviewFilters } from "./model.ts";
import "./overview.css";

function relativeTime(at: string | null | undefined, now: number): string {
  if (!at) return "Never";
  const minutes = Math.max(0, Math.floor((now - Date.parse(at)) / 60_000));
  if (!Number.isFinite(minutes)) return "Unknown";
  return minutes < 1 ? "Just now" : minutes < 60 ? `${minutes} min ago` : minutes < 1440 ? `${Math.floor(minutes / 60)} h ago` : `${Math.floor(minutes / 1440)} d ago`;
}

function Filters({ value, update, repositories }: { readonly value: OverviewFilters; readonly update: (patch: Partial<OverviewFilters>) => void; readonly repositories: readonly string[] }) {
  return <div className="gh-overview-filters">
    <Picker label="Repository" value={value.repo} onChange={(repo) => update({ repo })} data-testid="gh-filter-repo" size="sm"
      options={[{ id: "", label: "All repositories" }, ...repositories.map((id) => ({ id, label: id }))]} />
    <Picker label="Kind" value={value.kind} onChange={(kind) => update({ kind })} data-testid="gh-filter-kind" size="sm"
      options={[{ id: "", label: "Issues and PRs" }, { id: "issue", label: "Issues" }, { id: "pull_request", label: "Pull requests" }]} />
    <Picker label="State" value={value.state} onChange={(state) => update({ state })} data-testid="gh-filter-state" size="sm"
      options={[{ id: "", label: "All states" }, ...["open", "closed", "merged"].map((id) => ({ id, label: id }))]} />
  </div>;
}

function ObjectTable({ rows, now, linked }: { readonly rows: readonly ObjectRow[]; readonly now: number; readonly linked: boolean }) {
  const aether = useAether();
  return <div className="gh-object-table"><table><caption>Cached issues and pull requests</caption>
    <thead><tr>{["Object", "State", "Labels", "Author", "Updated", "Items", "Agents"].map((label) => <th key={label} scope="col">{label}</th>)}</tr></thead>
    <tbody>{rows.map(({ object, items }) => {
      const fact = object.ref.kind === "issue" ? issueFact(object) : pullRequestFact("Pull request", null, object);
      const agents = [...new Set(items.flatMap((item) => item.agents.map((use) => use.agent)))];
      return <tr key={objectId(object.ref)}><th scope="row"><a className="ui-link" href={objectHref(object.ref)}>#{object.ref.number} {object.title}</a><small>{object.ref.repo}</small></th>
        <td>{fact.status ? <StatusPill status={fact.status} /> : fact.text}</td>
        <td><div className="gh-overview-labels">{object.labels.map((label) => <Badge key={label}>{label}</Badge>)}</div></td>
        <td>{object.author ?? "Unknown"}</td><td><time dateTime={object.updated_at} title={object.updated_at}>{relativeTime(object.updated_at, now)}</time></td>
        <td>{linked ? items.length : "Unavailable"}</td><td><div className="gh-overview-labels">{agents.length === 0 ? "—" : agents.map((id) => {
          const found = aether.find(id);
          return found ? <a key={id} className="ui-link" href={routeHref("#/agents/:conn/:agent", { conn: found.conn, agent: id })}>{found.agent.name}</a> : <span key={id}>{id}</span>;
        })}</div></td></tr>;
    })}</tbody></table></div>;
}

function Relationships({ rows }: { readonly rows: readonly ObjectRow[] }) {
  const aether = useAether();
  const model = useMemo(() => {
    const graph = relationshipGraph(rows);
    return { ...graph, nodes: graph.nodes.map((node) => {
      const found = node.id.startsWith("agent:") ? aether.find(node.id.slice(6)) : null;
      return found ? { ...node, label: found.agent.name, status: found.agent.status.state } : node;
    }) };
  }, [rows, aether]);
  const layout = useMemo(() => relationshipLayout(model), [model]);
  const [selected, setSelected] = useState<string>();
  const node = model.nodes.find((entry) => entry.id === selected);
  const object = rows.find((row) => objectId(row.object.ref) === selected)?.object;
  return <section className="gh-relationships" aria-label="Relationships"><h2>From repository to agent</h2>
    <p>Choose a node to inspect it. Use arrow keys to move through the graph and Enter to select.</p>
    <div className="gh-overview-graph" data-testid="gh-graph"><GraphCanvas model={model} layout={layout} label="GitHub relationships" {...(selected === undefined ? {} : { selected })} onActivate={setSelected} /></div>
    <aside className="gh-node-detail" aria-live="polite">{node ? <><strong>{node.label}</strong><span>{node.status ?? "Linked"}</span>
      {object ? <a className="ui-link" href={objectHref(object.ref)}>Open GitHub object</a> : selected?.startsWith("item:") ? <a className="ui-link" href={`#/archon/items/${encodeURIComponent(selected.slice(5))}`}>Open item</a> : null}</>
      : "Select an issue, pull request, item, or agent to see its details."}</aside>
  </section>;
}

function Activity({ rows, now, linked }: { readonly rows: readonly ObjectRow[]; readonly now: number; readonly linked: boolean }) {
  const weeks = weeklyBuckets(rows, now);
  const slices = [{ label: "Open", value: rows.filter((row) => row.object.state === "open").length, tone: "success" as const },
    { label: "Closed", value: rows.filter((row) => row.object.state === "closed").length, tone: "neutral" as const },
    { label: "Merged", value: rows.filter((row) => row.object.state === "merged").length, tone: "accent" as const }];
  const prs = itemsPerPr(rows);
  return <section aria-label="Cached activity"><h2>Activity in the cache</h2><p>Current states grouped by their last update, over eight UTC weeks. These are cached snapshots, not event counts.</p>
    <div className="gh-overview-charts"><Donut label="States" slices={slices} />
      <Sparkline label="Open objects by update week" points={weeks.map((week) => ({ label: week.label, value: week.open }))} tone="success" />
      <Sparkline label="Closed objects by update week" points={weeks.map((week) => ({ label: week.label, value: week.closed }))} tone="neutral" />
      <Sparkline label="Merged PRs by update week" points={weeks.map((week) => ({ label: week.label, value: week.merged }))} />
      {linked && prs.length > 0 ? <BarChart label="Items per PR (first 12 cached PRs)" bars={prs.slice(0, 12)} /> : null}</div>
  </section>;
}

function CachedObjects({ info, repositories, refresh, now, onRefresh, interval }: {
  readonly info: ConnectionInfo; readonly repositories: readonly string[]; readonly refresh: string; readonly now: number;
  readonly onRefresh: () => void; readonly interval: number | null;
}) {
  const { value, update, clear } = useDraft(connectionDraftKey("github", info.id, "overview-filters"), DEFAULT_FILTERS);
  const data = useOverviewRows(info, repositories, refresh);
  const rows = filterRows(data.rows, value);
  if (!data.served) return <EmptyState title="GitHub cache unavailable">Update archond to a version that serves github_import, then refresh this page.</EmptyState>;
  return <>
    <Filters value={value} update={update} repositories={repositories} />
    {data.error ? <ErrorState title="Could not refresh GitHub data" message={data.error} onRetry={onRefresh} /> : null}
    {data.loading ? <div aria-label="Reading cached objects" aria-busy="true"><Skeleton height="3rem" /></div> : null}
    {!data.loading && !data.error && data.rows.length === 0 ? <EmptyState title="Nothing fetched yet" action={<Button onClick={onRefresh}>Refresh</Button>}>{interval === null ? "The cache has nothing for these repositories yet." : `The watcher polls every ${interval} s.`} Ask the operator to check the watched repositories if nothing arrives.</EmptyState> : null}
    {data.rows.length > 0 && rows.length === 0 ? <EmptyState title="No objects match" action={<Button onClick={clear}>Clear filters</Button>}>Choose another repository, kind, or state.</EmptyState> : null}
    {rows.length > 0 ? <><p>Showing {rows.length} cached objects; at most {OBJECTS_LIMIT} per repository.{data.linked ? "" : " This archond does not serve github_links; relationships are unavailable."}</p>
      <ObjectTable rows={rows} now={now} linked={data.linked} /><Relationships rows={rows} /><Activity rows={rows} now={now} linked={data.linked} /></> : null}
  </>;
}

/** archond keeps the watcher read for its CLI and refuses it to every remote device: a state, not a failure. */
export const keptForCli = (error: string): boolean => /not available from the web app/i.test(error);

const isRepo = (value: unknown): value is string => typeof value === "string" && /^[^/\s]+\/[^/\s]+$/.test(value);

/** The GitHub repositories the items link to (their issue, pull request or origin), sorted, each once. */
export function linkedRepositories(items: Readonly<Record<string, { readonly summary: unknown }>>): string[] {
  const repos = new Set<string>();
  for (const item of Object.values(items)) {
    const summary = item.summary as { issue?: { repo?: unknown } | null; pr?: { repo?: unknown } | null; origin?: { repository?: unknown } | null };
    for (const repo of [summary.issue?.repo, summary.pr?.repo, summary.origin?.repository]) if (isRepo(repo)) repos.add(repo);
  }
  invariant([...repos].every(isRepo), "every linked repository is owner/name");
  return [...repos].sort();
}

function WatcherTiles({ view, now }: { readonly view: archon.GithubWatcherView; readonly now: number }) {
  const lastPoll = view.repositories.flatMap((repo) => repo.last_poll_at ? [repo.last_poll_at] : []).sort((a, b) => Date.parse(a) - Date.parse(b)).at(-1);
  return <><div className="gh-overview-stats"><StatTile label="Watcher" value={view.enabled ? "On" : "Off"} tone={view.enabled ? "success" : "neutral"} />
    <StatTile label="Poll interval" value={`${view.poll_interval_secs} s`} /><StatTile label="Last poll" value={relativeTime(lastPoll, now)} />
    <StatTile label="Watched repositories" value={String(view.repositories.length)} /></div><ul className="gh-watched" aria-label="Watched repositories">
    {view.repositories.map((repo) => <li key={repo.repository}><strong>{repo.repository}</strong><Badge tone={repo.enabled ? "success" : "neutral"}>{repo.enabled ? "Watching" : "Disabled"}</Badge>
      <span>{repo.dispatched} dispatched · {repo.ignored} ignored · {relativeTime(repo.last_poll_at, now)}</span>{repo.last_error ? <p role="alert">{repo.last_error}</p> : null}</li>)}</ul></>;
}

/**
 * Where the overview reads from. The repositories are the watcher's, when it answers and is on,
 * and always the ones the items link to: archond keeps the watcher view for its CLI, or cannot
 * run one beside the operator's Aether, and the cache still serves the items' repositories.
 */
interface Source {
  readonly kind: "loading" | "watcher" | "items" | "failed";
  readonly view: archon.GithubWatcherView | undefined;
  readonly repositories: readonly string[];
  readonly error: string | null;
}

function source(watcher: ReadEntry | undefined, fromItems: readonly string[]): Source {
  const view = (watcher?.value ?? undefined) as archon.GithubWatcherView | undefined;
  const error = watcher?.error ?? null;
  if (error !== null && keptForCli(error)) return { kind: "items", view: undefined, repositories: fromItems, error: null };
  if (error !== null) return { kind: "failed", view, repositories: fromItems, error };
  if (view === undefined) return { kind: "loading", view, repositories: fromItems, error: null };
  invariant(Array.isArray(view.repositories), "a watcher view lists its repositories");
  const watched = view.enabled ? view.repositories.map((repo) => repo.repository) : [];
  return { kind: "watcher", view, repositories: [...new Set([...watched, ...fromItems])].sort(), error: null };
}

function OverviewBody({ info, from, now, refresh, reload }: { readonly info: ConnectionInfo; readonly from: Source; readonly now: number; readonly refresh: string; readonly reload: () => void }) {
  if (from.repositories.length > 0) {
    return <CachedObjects info={info} repositories={from.repositories} refresh={refresh} now={now} onRefresh={reload} interval={from.view?.enabled ? from.view.poll_interval_secs : null} />;
  }
  if (from.kind === "loading") return null;
  const watching = from.kind === "watcher" && from.view?.enabled === true;
  return <EmptyState title={watching ? "Nothing watched yet" : "No GitHub repository yet"} action={<a className="ui-link" href="#/archon/queue">Open the queue</a>}>
    Import an issue or link an item to a pull request; its repository's cached issues and pull requests then show here.{watching ? " The watcher lists what the archon CLI tells it to watch." : ""}
  </EmptyState>;
}

function Overview({ info }: { readonly info: ConnectionInfo }) {
  const change = useChangeKey(info.id);
  const [nonce, setNonce] = useState(0);
  const refresh = `${change}:${nonce}`;
  const watcher = useReread(info.id, "github:overview:watcher", { cmd: "github_watcher" }, ["github_watcher"], refresh);
  const items = useItems(info.id);
  const fromItems = useMemo(() => linkedRepositories(items), [items]);
  const from = source(watcher, fromItems);
  const now = useNow(STALE_CHECK_MS);
  const reload = () => setNonce((value) => value + 1);
  return <main className="gh-overview" aria-label="GitHub overview"><header><div><p className="gh-eyebrow">GitHub</p><h1>Overview</h1></div><Button onClick={reload}>Refresh</Button></header>
    {from.kind === "failed" ? <ErrorState title="Watcher unavailable" message={from.error ?? ""} onRetry={reload} /> : null}
    {from.kind === "loading" ? <Skeleton height="5rem" /> : null}
    {from.view ? <WatcherTiles view={from.view} now={now} /> : null}
    {from.kind === "items" ? <p className="gh-overview-note" role="note">The watcher reports to the archon CLI; this overview shows the cached issues and pull requests of the repositories your items link to.</p> : null}
    {from.kind === "watcher" && from.view?.enabled !== true && from.repositories.length > 0 ? <p className="gh-overview-note" role="note">The watcher is off; these are the cached issues and pull requests of the repositories your items link to.</p> : null}
    <OverviewBody info={info} from={from} now={now} refresh={refresh} reload={reload} />
  </main>;
}

export default function GithubOverviewScreen() {
  const info = useArchonConnection();
  return info?.state === "ready" ? <Overview info={info} /> : <EmptyState title="GitHub watcher unreachable" action={<a className="ui-link" href="#/settings">Open Settings</a>}>Connect an archond to browse its watched repositories and cached GitHub objects.</EmptyState>;
}
