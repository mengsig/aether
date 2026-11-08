import type { ArchonItem } from "@aether-ui/core";
import type { aether, ConnectionInfo } from "@aether-ui/protocol";
import { afterEach, describe, expect, test, vi } from "vitest";
import { joinReads } from "./derive.ts";
import { aetherInfo, archonInfo, BREAKDOWN, fixtureAgents, fixtureAttention, fixtureEvents, fixtureItems, LEARNER, NOW, WATCHER } from "./fixture/world.ts";
import {
  agentCounts, eventLabel, formatUsd, hourlyBuckets, HOUR_MS, masterySlices, needsYou, queueCounts, recentActivity, recentOutcomes,
  relativeTime, spendFromAgents, spendFromBreakdowns, spendSlices, startOfDay, watcherSummary,
} from "./model.ts";
import { nextStep, noesisState, presence, screenMode, type Facts } from "./steps.ts";

const minutesAgo = (minutes: number): string => new Date(NOW - minutes * 60_000).toISOString();

describe("time", () => {
  test("relative times step from minutes to hours to days, and keep text that is not a time", () => {
    expect(relativeTime(minutesAgo(0.5), NOW)).toBe("just now");
    expect(relativeTime(minutesAgo(-3), NOW)).toBe("just now");
    expect(relativeTime(minutesAgo(59), NOW)).toBe("59 min ago");
    expect(relativeTime(minutesAgo(61), NOW)).toBe("1 h ago");
    expect(relativeTime(minutesAgo(60 * 49), NOW)).toBe("2 d ago");
    expect(relativeTime("yesterday-ish", NOW)).toBe("yesterday-ish");
  });

  test("a day starts at local midnight", () => {
    const start = startOfDay(NOW);
    expect(new Date(start).getHours()).toBe(0);
    expect(new Date(start).getDate()).toBe(new Date(NOW).getDate());
    expect(() => startOfDay(Number.NaN)).toThrow(/real time/);
  });

  test("hourly buckets count each event in its clock hour, the current hour last", () => {
    const events = [0, 1, 59, 61, 60 * 11 + 59, 60 * 13].map((m, i) => ({ id: `e${i}`, seq: i + 1, at: minutesAgo(m), kind: { type: "agent.started" } }) as unknown as aether.Event);
    const buckets = hourlyBuckets(events, NOW);
    expect(buckets).toHaveLength(12);
    expect(buckets.at(-1)?.label).toBe(`${String(new Date(NOW).getHours()).padStart(2, "0")}:00`);
    const total = buckets.reduce((sum, bucket) => sum + bucket.value, 0);
    const current = Math.floor(NOW / HOUR_MS) * HOUR_MS;
    const expected = [0, 1, 59, 61, 60 * 11 + 59].filter((m) => NOW - m * 60_000 >= current - 11 * HOUR_MS).length;
    expect(total).toBe(expected);
    expect(() => hourlyBuckets(events, NOW, 0)).toThrow(/whole number of hours/);
  });
});

describe("hourly buckets across a daylight saving change", () => {
  afterEach(() => { vi.unstubAllEnvs(); });

  test("the clock hour that comes twice gets two labels", () => {
    vi.stubEnv("TZ", "America/New_York");
    // 2026-11-01: 02:00 EDT falls back to 01:00 EST, so 01:00 comes twice before 02:00 EST (07:00Z).
    const now = Date.UTC(2026, 10, 1, 7, 30);
    expect(new Date(now).getHours()).toBe(2);
    const labels = hourlyBuckets([], now).map((bucket) => bucket.label);
    expect(labels.slice(-4)).toEqual(["00:00", "01:00", "01:00 again", "02:00"]);
    expect(new Set(labels).size).toBe(labels.length);
  });
});

describe("counts by state", () =>{
  test("agents: running, waiting (waiting or ready) and needing attention (blocked or failed)", () => {
    expect(agentCounts(Object.values(fixtureAgents()))).toEqual({ total: 7, running: 2, waiting: 2, attention: 2 });
    expect(agentCounts([])).toEqual({ total: 0, running: 0, waiting: 0, attention: 0 });
  });

  test("needs you counts what the inbox shows: pending approvals only, stopped agents but not approval flags", () => {
    expect(needsYou([fixtureAttention(), null], 2)).toEqual({ questions: 1, approvals: 1, stopped: 1, archon: 2, total: 5 });
    expect(() => needsYou([], -1)).toThrow(/count/);
  });

  test("the queue folds dispatching into queued, and outcomes are the newest finished items", () => {
    const items = fixtureItems();
    const dispatching: ArchonItem = { ...(items.qi_queued as ArchonItem), state: "dispatching" };
    expect(queueCounts([...Object.values(items), dispatching])).toEqual({ queued: 2, running: 1, blocked: 1, total: 7 });
    expect(recentOutcomes(Object.entries(items)).map((o) => [o.id, o.kind])).toEqual([["qi_done", "succeeded"], ["qi_failed", "failed"], ["qi_old", "cancelled"]]);
    expect(recentOutcomes(Object.entries(items), 1)).toHaveLength(1);
  });
});

describe("spend today", () => {
  test("sums the daemons' breakdowns by model across connections", () => {
    const parsed = spendFromBreakdowns([BREAKDOWN, BREAKDOWN]);
    expect("value" in parsed && parsed.value).toMatchObject({ usd: 4.5, tokens: 2_700_000, complete: true, source: "breakdown" });
    expect("value" in parsed && parsed.value.byModel[0]).toEqual({ label: "claude-code/opus", value: 3 });
  });

  test("refuses a reply without rows or with a cost that is not an amount, and marks truncated as incomplete", () => {
    expect(spendFromBreakdowns([{}])).toEqual({ problem: "usage_breakdown has no rows" });
    expect(spendFromBreakdowns([{ ...BREAKDOWN, total: { ...BREAKDOWN.total, cost_usd: -1 } }])).toEqual({ problem: "usage_breakdown's total has a cost or token count that is not an amount" });
    const truncated = spendFromBreakdowns([{ ...BREAKDOWN, truncated: true }]);
    expect("value" in truncated && truncated.value.complete).toBe(false);
  });

  test("keeps subscription estimates separate from charges and rejects invalid estimates", () => {
    const usage = { ...BREAKDOWN.total, cost_usd: 0.2, estimated_cost_usd: 3, cost_complete: false, cost_basis: "mixed" as const };
    const parsed = spendFromBreakdowns([{ ...BREAKDOWN, total: usage }]);
    expect("value" in parsed && parsed.value).toMatchObject({ usd: 0.2, estimatedUsd: 3, listPrice: true, complete: false });
    expect(spendFromBreakdowns([{ ...BREAKDOWN, total: { ...usage, estimated_cost_usd: -1 } }])).toHaveProperty("problem");
    expect(spendFromBreakdowns([{ ...BREAKDOWN, rows: [null] }])).toHaveProperty("problem");
  });

  test("without a breakdown, counts only agents active since midnight", () => {
    const agents = Object.values(fixtureAgents());
    const stale = { ...fixtureAgents().agt_lead as aether.Agent, id: "agt_old", last_event_at: new Date(startOfDay(NOW) - 60_000).toISOString() };
    const spend = spendFromAgents([...agents, stale], startOfDay(NOW));
    expect(spend.usd).toBeCloseTo(2.26);
    expect(spend.byModel.map((point) => point.label)).toEqual(["claude-code/opus", "claude-code/sonnet", "claude-code/haiku"]);
  });

  test("the donut keeps three models in fixed tones and folds the rest into Other", () => {
    const points = ["a", "b", "c", "d", "e"].map((label, i) => ({ label, value: 5 - i }));
    expect(spendSlices(points).map((s) => [s.label, s.value, s.tone])).toEqual([["a", 5, "accent"], ["b", 4, "warning"], ["c", 3, "info"], ["Other", 3, "neutral"]]);
    expect(spendSlices(points.slice(0, 4)).map((s) => s.label)).toEqual(["a", "b", "c", "d"]);
    expect(spendSlices([{ label: "a", value: 0 }])).toEqual([]);
  });

  test("dollars show cents, and a fraction of a cent is not rounded to nothing", () => {
    expect(formatUsd(0)).toBe("$0.00");
    expect(formatUsd(0.004)).toBe("<$0.01");
    expect(formatUsd(12.345)).toBe("$12.35");
    expect(() => formatUsd(-1)).toThrow(/nonnegative/);
  });
});

describe("activity, learning and the watcher", () => {
  test("events read as words, newest first across connections, with agent names", () => {
    expect(eventLabel("agent.turn_completed")).toBe("Turn completed");
    expect(eventLabel("approval.requested")).toBe("Approval requested");
    expect(eventLabel("compaction")).toBe("Compaction");
    const agents = fixtureAgents();
    const other = fixtureEvents().slice(0, 2).map((event) => ({ ...event, at: minutesAgo(0.1) }));
    const rows = recentActivity([{ conn: "a", events: fixtureEvents(), agents }, { conn: "b", events: other, agents: {} }], 3);
    expect(rows.map((row) => [row.conn, row.name])).toEqual([["b", "agt_build"], ["b", "agt_lead"], ["a", "Daemon"]]);
  });

  test("mastery counts every concept once and refuses an unknown state", () => {
    const parsed = masterySlices(LEARNER);
    expect("value" in parsed && parsed.value.map((s) => [s.label, s.value])).toEqual([["Demonstrated", 3], ["Introduced", 2], ["Fragile", 1], ["Misconception", 1], ["Not seen yet", 1]]);
    expect(masterySlices({ concepts: [{ state: "mastered" }] })).toEqual({ problem: "a concept has an unknown state mastered" });
    expect(masterySlices(null)).toEqual({ problem: "noesis_learner carries no concepts" });
  });

  test("the watcher sums repositories across archonds and keeps the newest poll", () => {
    const parsed = watcherSummary([WATCHER, { ...WATCHER, enabled: false, repositories: [] }]);
    expect("value" in parsed && parsed.value).toEqual({ enabled: true, repos: 2, dispatched: 18, ignored: 4, lastPoll: minutesAgo(2), errors: 1 });
    expect(watcherSummary([{ enabled: true }])).toEqual({ problem: "github_watcher has no enabled flag or repositories" });
  });

  test("joined reads wait for every one, and fail on a failure with nothing held", () => {
    const entry = (value: unknown, error: string | null = null) => ({ value, asOf: null, issued: 1, requested: 1, settled: 1, error, errorIssued: error ? 1 : 0 });
    const count = (values: readonly unknown[]) => ({ value: values.length });
    expect(joinReads([entry(1), undefined], count)).toEqual({ state: "loading" });
    expect(joinReads([entry(null, "refused")], count)).toEqual({ state: "problem", problem: "refused" });
    expect(joinReads([entry(1), entry(2)], count)).toEqual({ state: "ready", value: 2 });
  });
});

describe("states and the next-step rule", () => {
  const conns = (...list: ConnectionInfo[]) => list;

  test("presence and the screen's mode follow the connections' states", () => {
    expect(presence(conns(), "aether")).toBe("none");
    expect(presence(conns(aetherInfo("backoff"), { ...aetherInfo(), id: "aether:two" }), "aether")).toBe("ready");
    expect(presence(conns(aetherInfo("pairing")), "aether")).toBe("connecting");
    expect(presence(conns(aetherInfo("closed")), "aether")).toBe("offline");
    expect(screenMode(conns())).toBe("empty");
    expect(screenMode(conns(aetherInfo("connecting"), archonInfo("closed")))).toBe("loading");
    expect(screenMode(conns(aetherInfo("closed")))).toBe("tiles");
    expect(screenMode(conns(aetherInfo("connecting"), archonInfo()))).toBe("tiles");
  });

  test("Noesis is unsupported without the feature, loading before its status, else on or off", () => {
    expect(noesisState(undefined, null)).toBe("none");
    expect(noesisState(aetherInfo("ready", []), null)).toBe("unsupported");
    expect(noesisState(aetherInfo(), null)).toBe("loading");
    expect(noesisState(aetherInfo(), { enabled: false })).toBe("off");
    expect(noesisState(aetherInfo("backoff"), { enabled: true })).toBe("offline");
    expect(() => noesisState(aetherInfo(), { enabled: "yes" })).toThrow(/enabled/);
  });

  const facts: Facts = { aether: "ready", archon: "ready", agents: 0, events: 0, items: 0, noesis: "off", sessions: 0, watched: 0 };

  test("an empty tile offers the step that fills it", () => {
    expect(nextStep("agents", facts)).toEqual({ label: "Start an agent", href: "#/activity" });
    expect(nextStep("queue", facts)).toEqual({ label: "Open the queue", href: "#/archon/queue" });
    expect(nextStep("learn", facts)).toEqual({ label: "Turn on Noesis", href: "#/learn" });
    expect(nextStep("learn", { ...facts, noesis: "on" })).toEqual({ label: "Start a session", href: "#/learn" });
    expect(nextStep("github", facts)).toEqual({ label: "Watch a repository", href: "#/github" });
    expect(nextStep("needs", facts)).toBeNull();
  });

  test("a tile with data, or whose daemon is still connecting, offers nothing", () => {
    const full = { ...facts, agents: 3, events: 9, items: 2, noesis: "on" as const, sessions: 1, watched: 1 };
    for (const tile of ["agents", "needs", "queue", "spend", "activity", "learn", "github"] as const) expect(nextStep(tile, full)).toBeNull();
    expect(nextStep("agents", { ...facts, aether: "connecting" })).toBeNull();
    expect(nextStep("learn", { ...facts, noesis: "loading" })).toBeNull();
  });

  test("a daemon that is not paired is paired first; one that is down sends you to the connections", () => {
    expect(nextStep("agents", { ...facts, aether: "none" })).toEqual({ label: "Connect to Aether", href: "#/pair/aether" });
    expect(nextStep("github", { ...facts, archon: "none" })).toEqual({ label: "Connect to Archon", href: "#/pair/archon" });
    expect(nextStep("queue", { ...facts, archon: "offline" })).toEqual({ label: "Check connections", href: "#/settings/connections" });
    expect(nextStep("needs", { ...facts, aether: "offline", archon: "none" })).toEqual({ label: "Check connections", href: "#/settings/connections" });
    expect(() => nextStep("agents", { ...facts, agents: -1 })).toThrow(/counts/);
  });
});
