// The bundle source for a repository catch-up: Archon's repository list, its items from core's store, the
// merged pull requests archond cached, and the repository's Aether memory, each read through core.

import { selectItems, useReread, useSelector, type ReadEntry } from "@aether-ui/core";
import type { aether, archon } from "@aether-ui/protocol";
import { useCallback, useMemo, useState } from "react";
import { useNoesisConnection } from "../learn/data.ts";
import { invariant } from "../learn/model.ts";
import { catchUpBundle } from "./catchup-bundle.ts";
import { isSince } from "./catchup-range.ts";
import { notConnected, useConnectionReady, type BundleRead } from "./read.ts";

export const REPOS_KEY = "noesis.catchup.repos";
const PR_LIMIT = 200;
const MEMORY_LIMIT = 40;

export interface GithubRepo {
  readonly host: string;
  /** `owner/name`, lowercased as Archon stores it. */
  readonly repo: string;
}

/** The GitHub repository most of this repository's items link to, or null when none links to GitHub. */
export function githubRepoOf(summaries: readonly archon.ItemSummary[]): GithubRepo | null {
  const counts = new Map<string, GithubRepo & { n: number }>();
  for (const summary of summaries) {
    for (const ref of [summary.pr, summary.issue]) {
      if (!ref) continue;
      const key = `${ref.host}/${ref.repo}`.toLowerCase();
      counts.set(key, { host: ref.host.toLowerCase(), repo: ref.repo.toLowerCase(), n: (counts.get(key)?.n ?? 0) + 1 });
    }
  }
  const [best] = [...counts.entries()].sort(([a, x], [b, y]) => y.n - x.n || a.localeCompare(b));
  return best === undefined ? null : { host: best[1].host, repo: best[1].repo };
}

/** The scope id Aether keys a repository's memories by: its normalized origin, `<host>/<owner>/<name>`. */
export const memoryScopeId = (github: GithubRepo): string => `${github.host}/${github.repo}`;

type Settled<T> = { readonly state: "loading" } | { readonly state: "error"; readonly message: string } | { readonly state: "ready"; readonly value: T };

/** A reread's answer: its value, else its error, else loading; a malformed value is an error. */
function settled<T>(entry: ReadEntry | undefined, what: string, parse: (value: unknown) => T | null): Settled<T> {
  if (entry?.value === null || entry?.value === undefined) return entry?.error ? { state: "error", message: entry.error } : { state: "loading" };
  const value = parse(entry.value);
  return value === null ? { state: "error", message: `archond or aetherd answered the ${what} read in an unexpected shape.` } : { state: "ready", value };
}

const asList = <T>(value: unknown): T[] | null => (Array.isArray(value) ? (value as T[]) : null);
const asObjects = (value: unknown): archon.GithubObject[] | null => asList<archon.GithubObject>((value as { objects?: unknown } | null)?.objects);

interface Reads {
  readonly repos: Settled<archon.Repo[]>;
  /** Null when the repository links to no GitHub repository, so there is nothing to read. */
  readonly pulls: Settled<archon.GithubObject[]> | null;
  readonly memories: Settled<aether.MemoryRecord[]> | null;
  readonly summaries: readonly archon.ItemSummary[];
  readonly retry: () => void;
}

function useReads(conn: string, id: string, ready: boolean): Reads {
  const [nonce, setNonce] = useState(0);
  const retry = useCallback(() => setNonce((n) => n + 1), []);
  const items = useSelector(useMemo(() => selectItems(conn), [conn]));
  const summaries = useMemo(
    () => Object.values(items).map((item) => item.summary as unknown as archon.ItemSummary).filter((summary) => summary.repo === id),
    [items, id],
  );
  const github = useMemo(() => githubRepoOf(summaries), [summaries]);
  const aetherConn = useNoesisConnection()?.id ?? "";
  const scope = github === null ? "" : memoryScopeId(github);
  const repos = useReread(conn, REPOS_KEY, ready ? { cmd: "repo_list" } : null, ["repos"], nonce);
  const prs = useReread(conn, `noesis.catchup.prs:${scope}`, ready && github ? { cmd: "list_github_objects", repo: github.repo, kind: "pull_request", state: "merged", limit: PR_LIMIT } : null, ["github_objects"], nonce);
  const query = { scope: { scope: "repository", id: scope }, limit: MEMORY_LIMIT, include_forgotten: false, include_superseded: false };
  const memories = useReread(aetherConn, `noesis.catchup.memories:${scope}`, aetherConn !== "" && github ? { cmd: "memories", query } : null, ["memories"], nonce);
  return useMemo(() => ({
    repos: settled(repos, "repository list", asList<archon.Repo>),
    pulls: github === null ? null : settled(prs, "pull request", asObjects),
    memories: github === null || aetherConn === "" ? null : settled(memories, "memory", asList<aether.MemoryRecord>),
    summaries,
    retry,
  }), [repos, prs, memories, github, aetherConn, summaries, retry]);
}

const UNREAD_NO_GITHUB = "no GitHub repository is known for this repository yet, so merged pull requests and Aether memory were not read";

/** The first read that failed or is still loading, as the source's answer; null when every read has answered. */
function firstPending(reads: Reads, retry: () => void): BundleRead | null {
  for (const read of [reads.repos, reads.pulls, reads.memories]) {
    if (read?.state === "error") return { state: "error", title: "Could not read the repository", message: read.message, retry };
    if (read?.state === "loading") return { state: "loading" };
  }
  return null;
}

export function useRepositorySource(conn: string, id: string, since?: string): BundleRead {
  invariant(id.length > 0, "a repository source names its repository");
  invariant(since !== undefined && isSince(since), "a catch-up names the day it starts from");
  const ready = useConnectionReady(conn, "archon");
  const reads = useReads(conn, id, ready);
  return useMemo((): BundleRead => {
    if (!ready) return notConnected("Archon", conn);
    const pending = firstPending(reads, reads.retry);
    if (pending !== null) return pending;
    const repo = reads.repos.state === "ready" ? reads.repos.value.find((entry) => entry.id === id) : undefined;
    if (repo === undefined) return { state: "missing", title: "No such repository", message: `${conn} has no registered repository ${id}.` };
    const catchUp = catchUpBundle({
      repoName: repo.name, since,
      pulls: reads.pulls?.state === "ready" ? reads.pulls.value : [],
      items: reads.summaries,
      memories: reads.memories?.state === "ready" ? reads.memories.value : [],
      unread: reads.pulls === null ? [UNREAD_NO_GITHUB] : [],
    });
    if (catchUp === null) return { state: "missing", title: "Nothing new", message: `${repo.name} has no merged pull requests or items updated since ${since}.` };
    return { state: "ready", bundle: catchUp.bundle, goal: catchUp.goal, notice: catchUp.notice };
  }, [ready, conn, id, since, reads]);
}
