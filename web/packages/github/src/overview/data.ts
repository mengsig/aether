import { useCommand, type ArchonItem, type Stamped } from "@aether-ui/core";
import { archon, type ConnectionInfo } from "@aether-ui/protocol";
import { useEffect, useMemo, useState } from "react";
import { OBJECTS_LIMIT, useItems } from "../issue/data.ts";
import { joinPage } from "../issue/join.ts";
import { invariant } from "../issue/ref.ts";
import { objectId, type ObjectRow } from "./model.ts";

type Send = ReturnType<typeof useCommand>;
interface Snapshot { readonly objects: readonly archon.GithubObject[]; readonly links: readonly archon.GithubLink[]; readonly details: Readonly<Record<string, Stamped>> }
const EMPTY: Snapshot = { objects: [], links: [], details: {} };

function replyData<T>(reply: Record<string, unknown>, tag: string): T {
  invariant(reply.reply === tag, `expected ${tag}, received ${String(reply.reply)}`);
  invariant(reply.data !== undefined && reply.data !== null, `${tag} includes data`);
  return reply.data as T;
}

async function batches<T, R>(values: readonly T[], read: (value: T) => Promise<R>, active: () => boolean): Promise<R[]> {
  const result: R[] = [];
  for (let i = 0; i < values.length && active(); i += 4) {
    const batch = await Promise.all(values.slice(i, i + 4).map(read));
    invariant(batch.length <= 4, "overview reads use at most four concurrent requests");
    result.push(...batch);
  }
  invariant(result.length <= values.length, "each input produces at most one result");
  return result;
}

async function readSnapshot(send: Send, repositories: readonly string[], linked: boolean, active: () => boolean): Promise<Snapshot> {
  const caches = await batches(repositories, async (repo) => {
    const data = replyData<{ objects: archon.GithubObject[] }>(await send({ ...archon.listGithubObjects({ repo, limit: OBJECTS_LIMIT }) }), "github_objects");
    invariant(Array.isArray(data.objects) && data.objects.length <= OBJECTS_LIMIT, "cached objects obey the read bound");
    invariant(data.objects.every((object) => ["open", "closed", "merged"].includes(object.state) && Number.isFinite(Date.parse(object.updated_at))), "cached objects have known states and valid update times");
    return data.objects;
  }, active);
  const objects = [...new Map(caches.flat().map((object) => [objectId(object.ref), object])).values()];
  if (!linked) return { objects, links: [], details: {} };
  const replies = await batches(objects, async (object) => {
    const data = replyData<{ links: archon.GithubLink[] }>(await send({ ...archon.issueItems({ ref: object.ref }) }), "issue_items");
    invariant(Array.isArray(data.links), "issue_items returns links");
    return data.links;
  }, active);
  const links = replies.flat();
  const ids = [...new Set(links.map((link) => link.item))];
  const details = await batches(ids, async (id): Promise<readonly [string, Stamped]> => {
    const detail = replyData<archon.ItemDetail>(await send({ cmd: "get_item", item: id }), "item_detail");
    invariant(detail.item.id === id, "an item detail belongs to its requested item");
    return [id, { value: detail as unknown as Record<string, unknown>, asOf: null }];
  }, active);
  return { objects, links, details: Object.fromEntries(details) };
}

export function useOverviewRows(info: ConnectionInfo, repositories: readonly string[], refresh: string) {
  const send = useCommand(info.id);
  const items = useItems(info.id);
  const scope = JSON.stringify([info.id, repositories]);
  const linked = info.features.includes("github_links");
  const served = info.features.includes("github_import");
  const [result, setResult] = useState({ scope: "", snapshot: EMPTY, completed: "", error: null as string | null });
  const request = `${scope}:${refresh}:${linked}`;
  useEffect(() => {
    if (info.state !== "ready" || !served) return;
    let active = true;
    const [, repos] = JSON.parse(scope) as [string, string[]];
    void readSnapshot(send, repos, linked, () => active).then(
      (snapshot) => { if (active) setResult({ scope, snapshot, completed: request, error: null }); },
      (cause: unknown) => { if (active) setResult((last) => ({ ...last, completed: request, error: cause instanceof Error ? cause.message : String(cause) })); },
    );
    return () => { active = false; };
  }, [send, info.state, served, scope, linked, request]);
  const snapshot = result.scope === scope ? result.snapshot : EMPTY;
  const rows = useMemo(() => joinSnapshot(snapshot, items), [snapshot, items]);
  return { rows, loading: result.completed !== request, error: result.completed === request ? result.error : null, served, linked };
}

function joinSnapshot(snapshot: Snapshot, items: Readonly<Record<string, ArchonItem>>): ObjectRow[] {
  const byObject = new Map<string, archon.GithubLink[]>();
  for (const link of snapshot.links) {
    const key = objectId(link.ref);
    const group = byObject.get(key) ?? [];
    group.push(link); byObject.set(key, group);
  }
  return snapshot.objects.map((object) => {
    const links = byObject.get(objectId(object.ref)) ?? [];
    const related = Object.fromEntries(links.flatMap((link) => {
      const item = items[link.item];
      return item ? [[link.item, item]] : [];
    }));
    return { object, items: joinPage({ page: object.ref, cached: [object], links, items: related, details: snapshot.details, itemLinks: {} }).items };
  });
}
