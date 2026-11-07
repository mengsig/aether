import type { aether, archon } from "@aether-ui/protocol";
import { describe, expect, test } from "vitest";
import { BUNDLE_LIMIT_BYTES, BUNDLE_MAX_ITEMS, checkBundle, utf8Bytes } from "./bundle.ts";
import { catchUpBundle, changedItemStatus, fitSections, type CatchUpInput } from "./catchup-bundle.ts";
import { MEMORIES, MERGED, PASSING, pull, REPO, SINCE, summary, SUMMARIES } from "./fixture/catchup.ts";

const INPUT: CatchUpInput = { repoName: REPO.name, since: SINCE, pulls: MERGED, items: SUMMARIES, memories: MEMORIES, unread: [] };
const refs = (bundle: aether.EvidenceBundle) => bundle.items.map((item) => `${item.ref.kind}:${item.ref.id}=${item.status}`);

function built(input: CatchUpInput = INPUT) {
  const result = catchUpBundle(input);
  if (result === null) throw new Error("the fixture has changes to catch up on");
  return result;
}

describe("the catch-up bundle", () => {
  test("keeps what changed since the day, newest first, then the memory most important first", () => {
    const { bundle, goal, notice } = built();
    expect(goal).toBe("Catch up on forge since 2026-11-01");
    expect(bundle.title).toBe(goal);
    expect(refs(bundle)).toEqual([
      "pr:github.com/acme/forge#212=verified", "pr:github.com/acme/forge#215=verified",
      "item:qi_lint=preliminary", "item:qi_cache=verified", "item:qi_retry=reviewed",
      "file:memory:mem_b=preliminary", "file:memory:mem_a=preliminary",
    ]);
    expect(notice).toBeNull();
    expect(checkBundle(bundle).ok).toBe(true);
  });

  test("only a merge record makes a PR verified: open or closed PRs are not read as merged work", () => {
    const open = pull(300, "Open work", "2026-11-05", { state: "open" });
    const closed = pull(301, "Closed work", "2026-11-05", { state: "closed" });
    const { bundle } = built({ ...INPUT, pulls: [open, closed, ...MERGED] });
    expect(bundle.items.filter((item) => item.ref.kind === "pr").map((item) => item.ref.id)).toEqual(["github.com/acme/forge#212", "github.com/acme/forge#215"]);
  });

  test("item summaries are preliminary unless their PR merged, passed checks or was approved", () => {
    const withPr = (patch: Partial<archon.PrRef>) => summary("qi_x", "X", "2026-11-04", { pr: { host: "github.com", repo: "acme/forge", kind: "pull_request", number: 1, ...patch } as archon.PrRef });
    expect(changedItemStatus(summary("qi_x", "X", "2026-11-04"))).toBe("preliminary");
    expect(changedItemStatus(withPr({ state: "open", checks: { state: "failure", failed: 2, total: 9 } }))).toBe("preliminary");
    expect(changedItemStatus(withPr({ state: "open", checks: PASSING }))).toBe("verified");
    expect(changedItemStatus(withPr({ state: "merged" }))).toBe("verified");
    expect(changedItemStatus(withPr({ state: "open", review_decision: "approved" }))).toBe("reviewed");
  });

  test("leaves out forgotten and superseded memory", () => {
    const text = built().bundle.items.map((item) => item.excerpt).join("\n");
    expect(text).not.toContain("An old approach");
    expect(text).not.toContain("Forgotten note");
  });

  test("is null when nothing changed in the range, even with memory", () => {
    expect(catchUpBundle({ ...INPUT, pulls: [MERGED[2] as archon.GithubObject], items: [SUMMARIES[3] as archon.ItemSummary] })).toBeNull();
  });

  test("says what could not be read", () => {
    const { notice } = built({ ...INPUT, pulls: [], memories: [], unread: ["no GitHub repository is known"] });
    expect(notice).toBe("Not read: no GitHub repository is known.");
  });
});

describe("fitting the daemon's limits", () => {
  const fact = (id: number, bytes: number): aether.EvidenceItem => ({ ref: { kind: "item", id: `qi_${id}` }, excerpt: "x".repeat(bytes), status: "preliminary" });

  test("takes sections in turn, so one cannot starve the others, and counts what is left out", () => {
    const big = Array.from({ length: 20 }, (_, index) => fact(index, 2_000));
    const small = Array.from({ length: 5 }, (_, index) => fact(100 + index, 100));
    const { kept, left } = fitSections([big, small]);
    expect(kept[1]).toHaveLength(5);
    expect(kept[0]?.length).toBeLessThan(20);
    expect(left).toEqual([20 - (kept[0]?.length ?? 0), 0]);
    expect(kept.flat().reduce((total, item) => total + utf8Bytes(item.excerpt), 0)).toBeLessThanOrEqual(BUNDLE_LIMIT_BYTES);
  });

  test("never passes 50 facts", () => {
    const many = Array.from({ length: 70 }, (_, index) => fact(index, 10));
    const { kept, left } = fitSections([many]);
    expect(kept[0]).toHaveLength(BUNDLE_MAX_ITEMS);
    expect(left).toEqual([20]);
  });

  test("a catch-up too large for one bundle keeps the newest and says how many were left out", () => {
    const pulls = Array.from({ length: 120 }, (_, index) => pull(1_000 + index, `Change ${index} ${"detail ".repeat(60)}`, "2026-11-04"));
    const { bundle, notice } = built({ ...INPUT, pulls, items: [], memories: [] });
    expect(checkBundle(bundle).ok).toBe(true);
    expect(bundle.items.length).toBeLessThanOrEqual(BUNDLE_MAX_ITEMS);
    expect(notice).toMatch(new RegExp(`^Left out to fit 32 KiB, the oldest and least important: ${120 - bundle.items.length} merged pull requests\\.$`));
  });
});
