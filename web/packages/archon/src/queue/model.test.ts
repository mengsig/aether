import type { ArchonItem } from "@aether-ui/core";
import { describe, expect, test } from "vitest";
import { demoWorld } from "./fixture/world.ts";
import { enqueueCommand } from "./NewItemDialog.tsx";
import { age, compareRows, FILTERS, inScope, NO_QUERY, queueRow, replyOf, safeHttpUrl, statePill, usd } from "./model.ts";

function stored(id: string, live: Partial<ArchonItem> = {}): ArchonItem {
  const summary = demoWorld().items.get(id)?.summary;
  if (!summary) throw new Error(`the demo world has ${id}`);
  const state = (summary.state as { state: string }).state;
  const reason = (summary.waiting_reason as { code?: string } | null)?.code;
  return { summary, state, ...(reason ? { waitingReason: reason } : {}), asOf: null, ...live };
}

describe("a queue row", () => {
  test("takes the live state over the summary's, and drops a reason for another code", () => {
    const row = queueRow("qi_rate_limit", stored("qi_rate_limit", { state: "running", waitingReason: "host_load" }));
    expect(row).toMatchObject({ kind: "running", waitingCode: "host_load", waitingText: "Host load is high", waitsOnYou: false });
    expect(queueRow("qi_rate_limit", stored("qi_rate_limit")).waitingText).toBe("Waiting for a free agent slot (4 of 4 busy)");
  });

  test("a gate, a failed dependency or a block waits on the operator; a finished item never does", () => {
    expect(queueRow("qi_pairing", stored("qi_pairing")).waitsOnYou).toBe(true);
    expect(queueRow("qi_landing", stored("qi_landing"))).toMatchObject({ waitsOnYou: true, blockedReason: "Dependency qi_assets failed" });
    expect(queueRow("qi_pairing", stored("qi_pairing", { state: "succeeded" }))).toMatchObject({ waitsOnYou: false, waitingText: null });
  });

  test("drops the summary's prompt and reason once the live reason has moved on", () => {
    const { summary, state } = stored("qi_pairing");
    const moved = queueRow("qi_pairing", { summary, state, stage: "build", asOf: 120 });
    expect(moved).toMatchObject({ waitsOnYou: false, waitingText: null, waitingCode: null });
    expect(queueRow("qi_pairing", stored("qi_pairing")).waitingText).toBe("Approve the proposal to shorten pairing codes to eight characters?");
  });

  test("rejects a state the queue does not know", () => {
    expect(() => queueRow("qi_pairing", stored("qi_pairing", { state: "merged" }))).toThrow(/known state/);
  });

  test("labels needs-you apart from blocked", () => {
    expect(statePill("running", true)).toEqual({ status: "operator", label: "Needs you" });
    expect(statePill("blocked", true)).toEqual({ status: "blocked", label: "Blocked" });
  });
});

describe("filtering and order", () => {
  const rows = [...demoWorld().items.keys()].map((id) => queueRow(id, stored(id)));

  test("each chip selects its states", () => {
    const pick = (id: string) => rows.filter(FILTERS.find((filter) => filter.id === id)?.test ?? (() => false)).map((row) => row.id).sort();
    expect(pick("waiting")).toEqual(["qi_landing", "qi_pairing"]);
    expect(pick("held")).toEqual(["qi_landing", "qi_pairing", "qi_rate_limit"]);
    expect(pick("failed")).toEqual(["qi_assets"]);
  });

  test("text search reads the repository name as well as the title, brief and id", () => {
    const names = new Map([["repo_api", "api"], ["repo_site", "site"]]);
    expect(rows.filter((row) => inScope(row, { ...NO_QUERY, text: "API" }, names)).map((row) => row.id).sort()).toEqual(["qi_pairing", "qi_rate_limit"]);
    expect(rows.filter((row) => inScope(row, { ...NO_QUERY, text: "qi_old" }, names)).map((row) => row.id)).toEqual(["qi_old_flag"]);
  });

  test("puts needs-you first, then running, then waiting by priority, then finished by recency", () => {
    expect([...rows].sort(compareRows).map((row) => row.id))
      .toEqual(["qi_pairing", "qi_landing", "qi_dark_mode", "qi_rate_limit", "qi_changelog", "qi_assets", "qi_old_flag"]);
  });
});

describe("the boundaries", () => {
  test("links only http and https", () => {
    expect(safeHttpUrl("https://example.com/r")).toBe("https://example.com/r");
    expect(safeHttpUrl("javascript:alert(1)")).toBeNull();
    expect(safeHttpUrl("not a url")).toBeNull();
  });

  test("reads a reply only under the tag it expects", () => {
    expect(replyOf({ reply: "status", data: 1 }, "status")).toBe(1);
    expect(() => replyOf({ reply: "item", data: 1 }, "item_detail")).toThrow("Expected a item_detail reply, got item");
  });

  test("an enqueue leaves an empty repository, pipeline and review to archond's defaults", () => {
    const command = enqueueCommand({ title: " T ", brief: "B", repo: "", pipeline: "", priority: "normal", review: "" }, "k1");
    expect(command).toEqual({ cmd: "enqueue", repo: null, pipeline: null, title: "T", brief: "B", priority: "normal", budget_usd: null, idempotency_key: "k1" });
    expect(() => enqueueCommand({ title: "", brief: "B", repo: "", pipeline: "", priority: "normal", review: "" }, "k1")).toThrow(/title and brief/);
  });

  test("formats money and ages", () => {
    expect([usd(0), usd(0.004), usd(3.185), usd(Number.NaN)]).toEqual(["$0.00", "<$0.01", "$3.19", "—"]);
    const now = Date.parse("2026-11-04T10:00:00Z");
    expect(["2026-11-04T10:00:20Z", "2026-11-04T09:15:00Z", "2026-11-04T05:00:00Z", "2026-11-01T10:00:00Z", "soon"].map((at) => age(at, now)))
      .toEqual(["now", "45 min", "5 h", "3 d", "unknown"]);
  });
});
