// A repository catch-up bundle: what changed since a day, from merged pull requests, Archon items and the
// repository's Aether memory. Pure: the source in catchup-source.ts passes in what core stored.

import type { aether, archon } from "@aether-ui/protocol";
import { invariant } from "../learn/model.ts";
import { prStatus } from "./archon-bundles.ts";
import { sinceInstant } from "./catchup-range.ts";
import { BUNDLE_LIMIT_BYTES, BUNDLE_MAX_ITEMS, bundleOf, evidence, utf8Bytes } from "./bundle.ts";
import { prEvidenceRef } from "./pr-bundle.ts";

export interface CatchUpInput {
  readonly repoName: string;
  /** `YYYY-MM-DD`: changes from the start of this UTC day on. */
  readonly since: string;
  readonly pulls: readonly archon.GithubObject[];
  readonly items: readonly archon.ItemSummary[];
  readonly memories: readonly aether.MemoryRecord[];
  /** What the source could not read, in words, so the notice says so. */
  readonly unread: readonly string[];
}

export interface CatchUp {
  readonly goal: string;
  readonly bundle: aether.EvidenceBundle;
  /** What was left out for size or not read; null when nothing was. */
  readonly notice: string | null;
}

const PR_BYTES = 700;
const ITEM_BYTES = 900;
const MEMORY_BYTES = 700;

function instant(stamp: string): number {
  const at = Date.parse(stamp);
  invariant(Number.isFinite(at), `${stamp} is a timestamp`);
  return at;
}

const day = (stamp: string): string => new Date(instant(stamp)).toISOString().slice(0, 10);
const newestFirst = <T>(stamp: (value: T) => string) => (a: T, b: T): number => instant(stamp(b)) - instant(stamp(a));

function mergedPrEvidence(object: archon.GithubObject): aether.EvidenceItem {
  const lines = [
    `Merged pull request #${object.ref.number} in ${object.ref.repo}: ${object.title}`,
    `Last updated ${day(object.updated_at)}${object.author ? ` · author ${object.author}` : ""}`,
    object.labels.length > 0 ? `Labels: ${object.labels.join(", ")}` : null,
  ];
  return evidence(prEvidenceRef(object.ref), prStatus(object.state, object.review_decision), lines.filter(Boolean).join("\n"), PR_BYTES);
}

function stateLine(state: archon.QueueState): string {
  switch (state.state) {
    case "succeeded": return `succeeded: ${state.summary}`;
    case "failed": return `failed: ${state.error}`;
    case "cancelled": return `cancelled${state.reason ? `: ${state.reason}` : ""}`;
    default: return state.state;
  }
}

/** A passing check or a merge on the item's PR verifies it, an approved review makes it reviewed; else preliminary. */
export function changedItemStatus(summary: archon.ItemSummary): aether.EvidenceStatus {
  const pr = summary.pr;
  if (!pr) return "preliminary";
  const checked = pr.checks !== null && pr.checks !== undefined && pr.checks.total > 0 && pr.checks.failed === 0 && pr.checks.state === "success";
  return checked ? "verified" : prStatus(pr.state, pr.review_decision);
}

function itemEvidence(summary: archon.ItemSummary): aether.EvidenceItem {
  const text = `Item: ${summary.title}\nState: ${stateLine(summary.state)} · updated ${day(summary.updated_at)}\n${summary.brief}`;
  return evidence({ kind: "item", id: summary.id }, changedItemStatus(summary), text, ITEM_BYTES);
}

/** The memory's id as a file-style reference: evidence references have no memory kind. */
function memoryEvidence(memory: aether.MemoryRecord): aether.EvidenceItem {
  const pinned = memory.pinned === true ? ", pinned" : "";
  return evidence({ kind: "file", id: `memory:${memory.id}` }, "preliminary", `Aether memory (${memory.kind}${pinned}): ${memory.text}`, MEMORY_BYTES);
}

const byImportance = (a: aether.MemoryRecord, b: aether.MemoryRecord): number =>
  Number(b.pinned === true) - Number(a.pinned === true) || b.importance - a.importance || instant(b.created_at) - instant(a.created_at);

const isLive = (memory: aether.MemoryRecord): boolean => !memory.forgotten_at && !memory.superseded_by && !memory.retracted;

export interface Fitted {
  /** Each section's kept items, in the order given. */
  readonly kept: readonly (readonly aether.EvidenceItem[])[];
  /** How many of each section were left out. */
  readonly left: readonly number[];
}

/**
 * Keeps ranked items from every section in turn, so no section starves the others, until the next one would
 * pass the daemon's byte or fact limit; the rest of that section is left out.
 */
export function fitSections(sections: readonly (readonly aether.EvidenceItem[])[]): Fitted {
  const taken = sections.map(() => 0);
  const open = sections.map(() => true);
  let bytes = 0;
  let count = 0;
  for (let round = 0; open.some(Boolean); round += 1) {
    sections.forEach((section, index) => {
      const next = open[index] ? section[round] : undefined;
      if (next === undefined) { open[index] = false; return; }
      const size = utf8Bytes(next.excerpt);
      if (bytes + size > BUNDLE_LIMIT_BYTES || count + 1 > BUNDLE_MAX_ITEMS) { open[index] = false; return; }
      bytes += size;
      count += 1;
      taken[index] = round + 1;
    });
  }
  invariant(bytes <= BUNDLE_LIMIT_BYTES && count <= BUNDLE_MAX_ITEMS, "what is kept fits the limits");
  return { kept: sections.map((section, index) => section.slice(0, taken[index])), left: sections.map((section, index) => section.length - (taken[index] ?? 0)) };
}

function noticeOf(left: readonly number[], unread: readonly string[]): string | null {
  const names = ["merged pull requests", "items", "memories"];
  const omitted = left.map((count, index) => (count > 0 ? `${count} ${names[index]}` : null)).filter((text) => text !== null);
  const parts = [
    omitted.length > 0 ? `Left out to fit ${Math.round(BUNDLE_LIMIT_BYTES / 1024)} KiB, the oldest and least important: ${omitted.join(", ")}.` : null,
    unread.length > 0 ? `Not read: ${unread.join("; ")}.` : null,
  ].filter((text) => text !== null);
  return parts.length === 0 ? null : parts.join(" ");
}

/** The catch-up for one repository, or null when nothing changed in the range. Merged PRs are the only ones read as merged. */
export function catchUpBundle(input: CatchUpInput): CatchUp | null {
  const from = sinceInstant(input.since);
  const pulls = input.pulls.filter((pull) => pull.ref.kind === "pull_request" && pull.state === "merged" && instant(pull.updated_at) >= from);
  const items = input.items.filter((item) => instant(item.updated_at) >= from);
  if (pulls.length + items.length === 0) return null;
  const sections = [
    pulls.sort(newestFirst((pull) => pull.updated_at)).map(mergedPrEvidence),
    items.sort(newestFirst((item) => item.updated_at)).map(itemEvidence),
    input.memories.filter(isLive).sort(byImportance).map(memoryEvidence),
  ];
  const { kept, left } = fitSections(sections);
  const goal = `Catch up on ${input.repoName} since ${input.since}`;
  const bundle = bundleOf(goal, kept.flat());
  invariant(bundle.items.length > 0, "a catch-up with changes keeps at least one fact");
  return { goal, bundle, notice: noticeOf(left, input.unread) };
}
