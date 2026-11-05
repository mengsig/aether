// The entity actions this package registers on `issue` and `pull_request` (CONTRACTS.md §2.5): open the
// GitHub page, and import an issue, which opens the page's preview so nothing is imported unconfirmed.

import { navigate } from "@aether-ui/core";
import type { Capabilities, EntityAction, EntityRef } from "@aether-ui/protocol";
import { pageHref, pageOfEntity } from "./ref.ts";

const isArchon = (ref: EntityRef): boolean => ref.conn.startsWith("archon:");

/** Whether `ref` names a page this package can show on a connected archond that tracks GitHub. */
export function canOpen(caps: Capabilities, ref: EntityRef): boolean {
  if (!isArchon(ref) || pageOfEntity(ref) === null || caps["archon.connected"] !== true) return false;
  return caps["archon.feature.github_links"] === true || caps["archon.feature.github_import"] === true;
}

/** Importing needs the import command served and a standing that may change Archon. */
export function canImport(caps: Capabilities, ref: EntityRef): boolean {
  return ref.kind === "issue" && canOpen(caps, ref) && caps["archon.feature.github_import"] === true && caps[`can.operate.${ref.conn}`] === true;
}

function open(ref: EntityRef, suffix = ""): void {
  const page = pageOfEntity(ref);
  if (page === null) throw new Error(`${ref.id} does not name a GitHub issue or pull request`);
  navigate(`${pageHref(page)}${suffix}`);
}

export const ACTIONS: readonly EntityAction[] = [
  { id: "d4.open-issue", kind: "issue", label: "Open issue page", requires: canOpen, run: (ref) => open(ref) },
  { id: "d4.open-pull-request", kind: "pull_request", label: "Open pull request page", requires: canOpen, run: (ref) => open(ref) },
  { id: "d4.import-issue", kind: "issue", label: "Import as Archon work…", requires: canImport, run: (ref) => open(ref, "?import") },
];

/** The page is these actions' target or renders them itself, so it never lists them among the others. */
export const OWN_ACTIONS: ReadonlySet<string> = new Set(ACTIONS.map((action) => action.id));
