import type { archon } from "@aether-ui/protocol";
import { describe, expect, test } from "vitest";
import { demoWorld } from "../queue/fixture/world.ts";
import { canCancel, canRetry, executionFact, issueFact, issueOf, objectFor, outcome, prFact, prFromUrl, prOf } from "./facts.ts";

const ref = (number: number, kind: archon.GithubKind = "issue"): archon.GithubRef => ({ host: "github.com", repo: "acme/site", number, kind });
const object = (number: number, kind: archon.GithubKind, state: archon.GithubObjectState, extra: Partial<archon.GithubObject> = {}): archon.GithubObject =>
  ({ ref: ref(number, kind), title: "t", state, labels: [], updated_at: "2026-11-04T09:00:00Z", fetched_at: "2026-11-04T09:00:00Z", ...extra });

function detailOf(id: string): archon.ItemDetail {
  const entry = demoWorld().items.get(id);
  if (!entry) throw new Error(`the demo world has ${id}`);
  return entry.detail as unknown as archon.ItemDetail;
}

describe("the three states", () => {
  test("a succeeded item with an open PR is not merged, and its issue stays open", () => {
    const pr: archon.PrRef = { ...ref(52, "pull_request"), state: "open", review_decision: "review_required", checks: { state: "success", total: 6, failed: 0 } };
    expect(executionFact("succeeded", detailOf("qi_changelog"), null)).toMatchObject({ status: "succeeded", text: "Succeeded" });
    expect(prFact(pr, null)).toEqual({ label: "Pull request", status: "pr-open", text: "#52 · Open", detail: "review required · 6 checks passed" });
    expect(issueFact(ref(38), object(38, "issue", "open"))).toMatchObject({ status: "issue-open", text: "#38 · Open" });
  });

  test("GitHub's read wins over the summary's PR state, and a draft open PR reads as draft", () => {
    const pr: archon.PrRef = { ...ref(57, "pull_request"), state: "open" };
    expect(prFact(pr, object(57, "pull_request", "merged")).status).toBe("pr-merged");
    expect(prFact(pr, object(57, "pull_request", "open", { draft: true })).text).toBe("#57 · Draft");
    expect(prFact(pr, object(57, "pull_request", "closed", { checks: { state: "failure", total: 4, failed: 1 } })).detail).toBe("1 of 4 checks failing");
  });

  test("an issue closed by its PR says why; an unread or missing one says so", () => {
    expect(issueFact(ref(41), object(41, "issue", "closed", { state_reason: "not_planned" }))).toMatchObject({ status: "issue-closed", detail: "acme/site · not planned" });
    expect(issueFact(ref(41), null)).toMatchObject({ status: null, text: "#41 · state not read" });
    expect(issueFact(null, null)).toMatchObject({ status: null, text: "Not linked" });
    expect(prFact(null, null)).toMatchObject({ status: null, text: "No pull request" });
  });

  test("finds the issue and PR in links when the summary has none, matching repos case-insensitively", () => {
    const links: archon.GithubLink[] = [
      { item: "qi_x", ref: ref(9), relation: "mentioned" as archon.LinkRelation, source: "watcher" as archon.LinkSource, created_at: "" },
      { item: "qi_x", ref: ref(7), relation: "origin", source: "watcher" as archon.LinkSource, created_at: "" },
      { item: "qi_x", ref: ref(8, "pull_request"), relation: "delivered" as archon.LinkRelation, source: "ship" as archon.LinkSource, created_at: "" },
    ];
    expect(issueOf(null, links)?.number).toBe(7);
    expect(prOf(null, links)?.number).toBe(8);
    expect(objectFor({ ...ref(7), repo: "Acme/Site" }, [object(7, "issue", "open")])?.state).toBe("open");
    expect(objectFor(ref(7, "pull_request"), [object(7, "issue", "open")])).toBeNull();
  });

  test("falls back to a protocol 23 pr_url, and only one that names a pull request over http", () => {
    const summary = { pr: null, pr_url: "https://github.com/acme/site/pull/57" } as unknown as archon.ItemSummary;
    expect(prOf(summary, [])).toEqual({ host: "github.com", repo: "acme/site", number: 57, kind: "pull_request" });
    expect(prFact(prOf(summary, []), null).text).toBe("#57 · state unknown");
    expect(["javascript:alert(1)//github.com/a/b/pull/1", "https://github.com/acme/site/issues/5", "https://github.com/acme/pull/5", null].map(prFromUrl))
      .toEqual([null, null, null, null]);
  });
});

describe("what holds an item", () => {
  test("names the outcome or what the item waits on", () => {
    expect(outcome(detailOf("qi_assets"))).toEqual({ tone: "danger", title: "Failed", text: "The build stage timed out after 120 minutes." });
    expect(outcome(detailOf("qi_pairing"))).toMatchObject({ tone: "warning", title: "Waiting on you · Proposal" });
    expect(outcome(detailOf("qi_rate_limit"))).toMatchObject({ tone: "neutral", title: "Waiting" });
    expect(outcome(detailOf("qi_dark_mode"))).toBeNull();
  });

  test("offers cancel while unfinished and retry once failed, cancelled or blocked", () => {
    expect((["queued", "running", "blocked", "succeeded", "failed", "cancelled"] as const).map((kind) => [canCancel(kind), canRetry(kind)]))
      .toEqual([[true, false], [true, false], [true, true], [false, false], [false, true], [false, true]]);
  });

  test("a gate waiting on the operator reads as needs you, with the reason", () => {
    expect(executionFact("running", detailOf("qi_pairing"), "plan")).toEqual({
      label: "Execution", status: "operator", text: "Needs you", detail: "stage plan · Waiting on your gate decision for the proposal",
    });
  });
});
