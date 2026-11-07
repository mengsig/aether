// Bundle sources for Archon entities: each rereads through core's manager or core's generic reread
// and builds from what core stored; nothing here talks to a daemon itself.

import { selectItem, selectItemDetail, selectRunPlan, useReread, useSelector, type State } from "@aether-ui/core";
import type { aether, archon } from "@aether-ui/protocol";
import { useCallback, useMemo, useState } from "react";
import { invariant } from "../learn/model.ts";
import { itemBundle, runBundle } from "./archon-bundles.ts";
import { findPr, parsePrId, prBundle, type PrId } from "./pr-bundle.ts";
import { notConnected, useConnectionReady, useCoreRead, useShownItem, type BundleRead, type Reader } from "./read.ts";

const refreshItem: Reader = (app, conn, [item]) => app.manager.refreshItem(conn, item ?? "");
const readPlan: Reader = (app, conn, [item, run]) => app.manager.readPlan(conn, item ?? "", run ?? "");

function settledRead(ready: boolean, conn: string, read: { error: string | null; settled: boolean; retry: () => void }, what: string): BundleRead {
  if (!ready) return notConnected("Archon", conn);
  if (read.error !== null) return { state: "error", title: `Could not read ${what}`, message: read.error, retry: read.retry };
  return { state: "loading" };
}

export function useItemSource(conn: string, id: string): BundleRead {
  invariant(id.length > 0, "an item source names its item");
  const ready = useConnectionReady(conn, "archon");
  const stamped = useSelector(useMemo(() => selectItemDetail(conn, id), [conn, id]));
  const stored = useSelector(useMemo(() => selectItem(conn, id), [conn, id]));
  useShownItem(conn, ready, id);
  const read = useCoreRead(conn, ready, [id], refreshItem);
  const bundle = useMemo(() => {
    if (stamped === undefined) return null;
    const detail = stamped.value as unknown as archon.ItemDetail;
    invariant(detail.item.id === id, `the detail of ${id} describes ${detail.item.id}`);
    return itemBundle(detail, (stored?.summary ?? null) as archon.ItemSummary | null);
  }, [stamped, stored, id]);
  return bundle === null ? settledRead(ready, conn, read, "the item") : { state: "ready", bundle };
}

/** The item a run belongs to, from a stored detail that lists it or its stored plan. */
function owningItem(state: State, conn: string, run: string): string | null {
  const archonState = state.archon[conn];
  if (archonState === undefined) return null;
  for (const [item, stamped] of Object.entries(archonState.details)) {
    if ((stamped.value as unknown as archon.ItemDetail).runs.some((entry) => entry.id === run)) return item;
  }
  return archonState.plans[run]?.item ?? null;
}

export function useRunSource(conn: string, id: string): BundleRead {
  const ready = useConnectionReady(conn, "archon");
  const found = useSelector(useCallback((state: State) => owningItem(state, conn, id), [conn, id]));
  // The owner, once seen, is kept: it stays on screen and is reread even while its detail is refetched.
  const [seen, setSeen] = useState<string | null>(found);
  if (found !== null && found !== seen) setSeen(found);
  const item = found ?? seen;
  const servesPlans = useSelector(useCallback((state: State) => state.connections[conn]?.features.includes("run_plan") === true, [conn]));
  const stamped = useSelector(useMemo(() => selectItemDetail(conn, item ?? ""), [conn, item]));
  const plan = useSelector(useMemo(() => selectRunPlan(conn, id), [conn, id]));
  useShownItem(conn, ready, item);
  const read = useCoreRead(conn, ready, item === null ? null : [item], refreshItem);
  const planRead = useCoreRead(conn, ready && servesPlans, item === null ? null : [item, id], readPlan);
  const bundle = useMemo(() => {
    if (stamped === undefined) return null;
    const detail = stamped.value as unknown as archon.ItemDetail;
    if (!detail.runs.some((entry) => entry.id === id)) return null;
    return runBundle(detail, id, plan ? (plan.value as unknown as archon.RunPlan) : null);
  }, [stamped, plan, id]);
  if (item === null) return { state: "missing", title: "Open the run first", message: "This run's item is not loaded here. Open the run from its item, then choose Learn from this again." };
  if (bundle !== null) return servesPlans && plan === undefined && !planRead.settled ? { state: "loading" } : { state: "ready", bundle };
  return settledRead(ready, conn, read, "the run");
}

interface GithubObjects { readonly objects: archon.GithubObject[] }

/** What a `github_objects` reply says about one PR: a malformed reply, not cached, or its bundle. */
type PrLookup = { readonly shape: "malformed" } | { readonly shape: "absent" } | { readonly shape: "found"; readonly bundle: aether.EvidenceBundle };

function lookUpPr(value: unknown, pr: PrId): PrLookup {
  const objects = (value as Partial<GithubObjects> | null)?.objects;
  if (!Array.isArray(objects)) return { shape: "malformed" };
  const object = findPr(objects, pr);
  return object === null ? { shape: "absent" } : { shape: "found", bundle: prBundle(object) };
}

const PR_UNREAD = "Could not read the pull request";

export function usePullRequestSource(conn: string, id: string): BundleRead {
  const pr = useMemo(() => parsePrId(id), [id]);
  const ready = useConnectionReady(conn, "archon");
  const [nonce, setNonce] = useState(0);
  const command = pr === null ? null : { cmd: "list_github_objects", repo: pr.repo, kind: "pull_request" };
  const entry = useReread(conn, `noesis.understand.prs:${pr?.repo ?? ""}`, command, ["github_objects"], nonce);
  const retry = useCallback(() => setNonce((n) => n + 1), []);
  const value = entry?.value ?? null;
  const lookup = useMemo(() => (pr === null || value === null ? null : lookUpPr(value, pr)), [value, pr]);
  if (pr === null) return { state: "error", title: "Not a pull request", message: `"${id}" does not name a pull request as <host>/<owner>/<repo>#<number>.` };
  if (lookup === null) {
    if (entry?.error) return { state: "error", title: PR_UNREAD, message: entry.error, retry };
    return ready ? { state: "loading" } : notConnected("Archon", conn);
  }
  switch (lookup.shape) {
    case "malformed": return { state: "error", title: PR_UNREAD, message: "archond answered list_github_objects without its objects." };
    case "absent": return { state: "missing", title: "Pull request not cached", message: `Archon has not cached ${pr.repo}#${pr.number} yet.` };
    case "found": return { state: "ready", bundle: lookup.bundle };
  }
}
