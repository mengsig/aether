// The entities the command palette finds, beyond screens: agents, Archon items, initiatives, pull
// requests and Learn sessions. Core's store is read here, where the packages meet, so ui stays generic.

import { routeHref, type ArchonItem, type State } from "@aether-ui/core";
import type { aether, archon, Capabilities, ConnectionInfo, ConnId, RouteDef } from "@aether-ui/protocol";
import type { Command } from "@aether-ui/ui";

type ByConn<T> = Readonly<Record<ConnId, T>>;

/** Only the store slices the palette reads, each a reference the store keeps while it is unchanged. */
export interface PaletteSources {
  readonly connections: ByConn<ConnectionInfo>;
  readonly agents: ByConn<Readonly<Record<string, aether.Agent>>>;
  readonly sessions: ByConn<Readonly<Record<string, aether.NoesisSession>>>;
  readonly items: ByConn<Readonly<Record<string, ArchonItem>>>;
  readonly initiatives: ByConn<readonly archon.Initiative[]>;
}

const NO_INITIATIVES: readonly archon.Initiative[] = Object.freeze([]);

const ROUTES = {
  agent: "#/agents/:conn/:agent",
  item: "#/archon/items/:item",
  initiative: "#/archon/initiatives/:id",
  pullRequest: "#/github/:host/:owner/:repo/:number",
  session: "#/learn/sessions/:id",
} as const;

function invariant(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(`palette entities: ${message}`);
}

function mapConns<S, T>(states: ByConn<S>, pick: (state: S) => T): ByConn<T> {
  return Object.fromEntries(Object.entries(states).map(([conn, state]) => [conn, pick(state)]));
}

export function selectPaletteSources(state: State): PaletteSources {
  return {
    connections: state.connections,
    agents: mapConns(state.aether, (conn) => conn.agents),
    sessions: mapConns(state.aether, (conn) => conn.learn.sessions),
    items: mapConns(state.archon, (conn) => conn.items),
    initiatives: mapConns(state.archon, (conn) => (conn.graph?.initiatives as unknown as readonly archon.Initiative[] | undefined) ?? NO_INITIATIVES),
  };
}

function sameRefs(a: ByConn<unknown>, b: ByConn<unknown>): boolean {
  const keys = Object.keys(a);
  return keys.length === Object.keys(b).length && keys.every((key) => Object.is(a[key], b[key]));
}

/** Whether two reads hold the same slices, so a change elsewhere in the store rebuilds nothing. */
export function sameSources(a: PaletteSources, b: PaletteSources): boolean {
  return sameRefs(a.connections, b.connections) && sameRefs(a.agents, b.agents) && sameRefs(a.sessions, b.sessions)
    && sameRefs(a.items, b.items) && sameRefs(a.initiatives, b.initiatives);
}

export interface EntityTarget {
  readonly navigate: (href: string) => void;
  readonly offered: (pattern: string) => boolean;
}

function go(target: EntityTarget, id: string, href: string): Pick<Command, "id" | "run"> {
  return { id, run: () => target.navigate(href) };
}

function agentCommands(sources: PaletteSources, target: EntityTarget): Command[] {
  if (!target.offered(ROUTES.agent)) return [];
  const several = Object.keys(sources.agents).length > 1;
  return Object.entries(sources.agents).flatMap(([conn, agents]) => Object.values(agents).map((agent): Command => {
    const where = sources.connections[conn]?.label ?? conn;
    const status = agent.archived ? "archived" : agent.status.state;
    return {
      ...go(target, `agent:${conn}:${agent.id}`, routeHref(ROUTES.agent, { conn, agent: agent.id })),
      label: agent.name, group: "Agent", subtitle: [agent.role, status, ...(several ? [where] : [])].join(" · "),
      keywords: [agent.id, agent.role, where],
    };
  }));
}

function itemSummary(id: string, item: ArchonItem): archon.ItemSummary {
  const summary = item.summary as unknown as archon.ItemSummary;
  invariant(typeof summary.title === "string", `item ${id} has a title`);
  return summary;
}

function itemCommands(sources: PaletteSources, target: EntityTarget): Command[] {
  if (!target.offered(ROUTES.item)) return [];
  return Object.entries(sources.items).flatMap(([conn, items]) => Object.entries(items).map(([id, item]): Command => {
    const summary = itemSummary(id, item);
    const repo = summary.repo ?? null;
    return {
      ...go(target, `item:${conn}:${id}`, routeHref(ROUTES.item, { item: id })),
      label: summary.title, group: "Item", subtitle: [item.state, item.stage, repo].filter(Boolean).join(" · "),
      keywords: [id, summary.pipeline, ...(repo ? [repo] : [])],
    };
  }));
}

function initiativeCommands(sources: PaletteSources, target: EntityTarget): Command[] {
  if (!target.offered(ROUTES.initiative)) return [];
  return Object.entries(sources.initiatives).flatMap(([conn, initiatives]) => initiatives.map((initiative): Command => {
    invariant(typeof initiative.id === "string" && typeof initiative.title === "string", `an initiative on ${conn} has an id and a title`);
    return {
      ...go(target, `initiative:${conn}:${initiative.id}`, routeHref(ROUTES.initiative, { id: initiative.id })),
      label: initiative.title, group: "Initiative", subtitle: initiative.state, keywords: [initiative.id],
    };
  }));
}

/** One command per distinct pull request, named for the first item that carries it. */
function pullRequestCommands(sources: PaletteSources, target: EntityTarget): Command[] {
  if (!target.offered(ROUTES.pullRequest)) return [];
  const found = new Map<string, Command>();
  for (const items of Object.values(sources.items)) {
    for (const [id, item] of Object.entries(items)) {
      const summary = itemSummary(id, item);
      const pr = summary.pr;
      if (!pr) continue;
      const [owner, repo, ...rest] = pr.repo.split("/");
      invariant(owner && repo && rest.length === 0, `a pull request repository is owner/name, not ${pr.repo}`);
      const key = `pr:${pr.host}/${pr.repo}#${pr.number}`.toLowerCase();
      if (found.has(key)) continue;
      found.set(key, {
        ...go(target, key, routeHref(ROUTES.pullRequest, { host: pr.host, owner, repo, number: String(pr.number) })),
        label: `${pr.repo}#${pr.number}`, group: "Pull request", subtitle: [pr.state ?? "open", summary.title].join(" · "),
        keywords: [id, String(pr.number)],
      });
    }
  }
  return [...found.values()];
}

function sessionCommands(sources: PaletteSources, target: EntityTarget): Command[] {
  if (!target.offered(ROUTES.session)) return [];
  return Object.entries(sources.sessions).flatMap(([conn, sessions]) => Object.values(sessions).map((session): Command => ({
    ...go(target, `session:${conn}:${session.id}`, routeHref(ROUTES.session, { id: session.id })),
    label: session.goal, group: "Learn session", subtitle: `${session.mode} · ${session.phase}`, keywords: [session.id, session.agent],
  })));
}

/** Whether the app registered a screen at `pattern` that this build's capabilities allow. */
export function routeOffered(routes: readonly RouteDef[], capabilities: Capabilities, pattern: string): boolean {
  const route = routes.find((candidate) => candidate.path === pattern);
  return route !== undefined && route.requires?.(capabilities) !== false;
}

/** Palette commands for every entity in `sources` whose screen exists; each navigates to its own route. */
export function entityCommands(sources: PaletteSources, target: EntityTarget): Command[] {
  const commands = [
    ...agentCommands(sources, target), ...itemCommands(sources, target), ...initiativeCommands(sources, target),
    ...pullRequestCommands(sources, target), ...sessionCommands(sources, target),
  ];
  invariant(new Set(commands.map((command) => command.id)).size === commands.length, "entity command ids are unique");
  return commands;
}
