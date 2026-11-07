// The entity actions this package registers on `issue` and `pull_request` (CONTRACTS.md §2.5): open the
// GitHub page, and import an issue, which opens the page's preview so nothing is imported unconfirmed.

import { currentHash, matchPattern, navigate } from "@aether-ui/core";
import type { Capabilities, EntityAction, EntityRef } from "@aether-ui/protocol";
import { invariant, PAGE_ROUTE, pageHref, pageOfEntity, parsePage, samePlace } from "./ref.ts";

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

/** Whether `ref`'s own page is open: it renders the import itself and needs no link to itself. */
export function onItsPage(ref: EntityRef, hash: string): boolean {
  invariant(hash.startsWith("#"), `a location hash starts with #, got ${hash}`);
  invariant(ref.kind === "issue" || ref.kind === "pull_request", `a GitHub page shows an issue or PR, not a ${ref.kind}`);
  const params = matchPattern(PAGE_ROUTE, hash);
  const here = params ? parsePage(params) : null;
  const page = pageOfEntity(ref);
  return here !== null && page !== null && samePlace(here, page);
}

const elsewhere = (allowed: (caps: Capabilities, ref: EntityRef) => boolean) =>
  (caps: Capabilities, ref: EntityRef): boolean => allowed(caps, ref) && !onItsPage(ref, currentHash());

function open(ref: EntityRef, suffix = ""): void {
  const page = pageOfEntity(ref);
  if (page === null) throw new Error(`${ref.id} does not name a GitHub issue or pull request`);
  navigate(`${pageHref(page)}${suffix}`);
}

export const ACTIONS: readonly EntityAction[] = [
  { id: "d4.open-issue", kind: "issue", label: "Open issue page", requires: elsewhere(canOpen), run: (ref) => open(ref) },
  { id: "d4.open-pull-request", kind: "pull_request", label: "Open pull request page", requires: elsewhere(canOpen), run: (ref) => open(ref) },
  { id: "d4.import-issue", kind: "issue", label: "Import as Archon work…", requires: elsewhere(canImport), run: (ref) => open(ref, "?import") },
];
