import type { EntityKind } from "@aether-ui/protocol";
import { invariant } from "../learn/invariant.ts";

/** What a bundle is built from, an entity or a repository's catch-up; each has a source in sources.ts. */
export const UNDERSTAND_KINDS = ["item", "run", "agent", "pull_request", "repository"] as const satisfies readonly (EntityKind | "repository")[];
export type UnderstandKind = (typeof UNDERSTAND_KINDS)[number];

export interface UnderstandRequest {
  readonly kind: UnderstandKind;
  /** The entity's own connection: an Archon one for items, runs and PRs, an Aether one for agents. */
  readonly conn: string;
  readonly id: string;
  /** A repository catch-up's first day, `YYYY-MM-DD`; set for `repository` and no other kind. */
  readonly since?: string;
}

export const QUERY_KEY = "understand";

/** `#/learn?understand=<kind>&conn=<conn>&id=<id>[&since=<day>]`: routes are frozen, so the sheet rides on F5's `#/learn`. */
export function understandHref(request: UnderstandRequest): string {
  invariant(request.conn.length > 0 && request.id.length > 0, "an understand link names a connection and an entity");
  invariant((request.since !== undefined) === (request.kind === "repository"), "only a repository catch-up has a first day");
  const query = new URLSearchParams({ [QUERY_KEY]: request.kind, conn: request.conn, id: request.id });
  if (request.since !== undefined) query.set("since", request.since);
  return `#/learn?${query.toString()}`;
}

