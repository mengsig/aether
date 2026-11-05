// The GitHub object a page names: parsed from `#/github/:host/:owner/:repo/:number`, and as the
// `EntityRef.id` of an `issue` or `pull_request` (`<host>/<owner>/<repo>#<number>`, the import key's form).

import { routeHref } from "@aether-ui/core";
import type { archon, EntityRef } from "@aether-ui/protocol";

export const PAGE_ROUTE = "#/github/:host/:owner/:repo/:number";

export function invariant(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(`Invariant failed: ${message}`);
}

/** An issue or pull request by place and number; GitHub numbers both from one sequence per repository. */
export interface PageRef {
  readonly host: string;
  /** `owner/name`, as typed in the route. */
  readonly repo: string;
  readonly number: number;
}

const HOST = /^[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?(?::\d{1,5})?$/i;
const NAME = /^[\w.-]{1,100}$/;
const NUMBER = /^[1-9]\d{0,15}$/;

/** The page's object from route params, or null when they do not name one (a hand-typed or stale link). */
export function parsePage(params: Readonly<Record<string, string>>): PageRef | null {
  const { host, owner, repo, number } = params;
  if (!host || !owner || !repo || !number) return null;
  if (!HOST.test(host) || !NAME.test(owner) || !NAME.test(repo) || !NUMBER.test(number)) return null;
  if (owner === "." || owner === ".." || repo === "." || repo === "..") return null;
  const parsed = Number(number);
  if (!Number.isSafeInteger(parsed)) return null;
  return { host: host.toLowerCase(), repo: `${owner}/${repo}`, number: parsed };
}

export function githubRef(page: PageRef, kind: archon.GithubKind): archon.GithubRef {
  invariant(kind === "issue" || kind === "pull_request", `a GitHub kind is issue or pull_request, not ${String(kind)}`);
  return { host: page.host, repo: page.repo, number: page.number, kind };
}

type Place = Pick<PageRef, "host" | "repo" | "number">;

/** Whether `ref` is the page's object, of any kind: GitHub repositories and hosts compare without case. */
export function samePlace(ref: Place, page: Place): boolean {
  return ref.host.toLowerCase() === page.host.toLowerCase() && ref.repo.toLowerCase() === page.repo.toLowerCase() && ref.number === page.number;
}

export function pageHref(ref: Place): string {
  const [owner, repo, ...rest] = ref.repo.split("/");
  invariant(owner && repo && rest.length === 0, `a GitHub repository is owner/name, not ${ref.repo}`);
  invariant(Number.isSafeInteger(ref.number) && ref.number > 0, `a GitHub number is a positive integer, not ${ref.number}`);
  return routeHref(PAGE_ROUTE, { host: ref.host, owner, repo, number: String(ref.number) });
}

/** The page's object on GitHub itself; only for an https host this page parsed. */
export function githubUrl(page: PageRef, kind: archon.GithubKind | null): string {
  invariant(HOST.test(page.host), `${page.host} is a host name`);
  const path = kind === "pull_request" ? "pull" : "issues";
  return `https://${page.host}/${page.repo}/${path}/${page.number}`;
}

export function entityId(page: PageRef): string {
  return `${page.host}/${page.repo}#${page.number}`;
}

const ENTITY_ID = /^([^/#]+)\/([^/#]+)\/([^/#]+)#(\d+)$/;

/** The page an entity's id names; null for an id this package did not make. */
export function pageOfEntity(ref: EntityRef): PageRef | null {
  if (ref.kind !== "issue" && ref.kind !== "pull_request") return null;
  const match = ENTITY_ID.exec(ref.id);
  if (!match) return null;
  const [, host, owner, repo, number] = match;
  invariant(host !== undefined && owner !== undefined && repo !== undefined && number !== undefined, "the entity id pattern captures four parts");
  return parsePage({ host, owner, repo, number });
}
