// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { HomeScreen } from "./HomeScreen.tsx";
import { HOME_REFRESH_MS } from "./sources.ts";
import { harness, type Harness, type Replies } from "./fixture/harness.test-util.tsx";
import { aetherInfo, archonInfo, BREAKDOWN, fixtureEvents, QUESTIONS, NOW, seedWorld, WATCHER, AETHER_CONN, ARCHON_CONN } from "./fixture/world.ts";
import type { ConnectionInfo } from "@aether-ui/protocol";

const REPLIES: Replies = { github_watcher: WATCHER, list_questions: QUESTIONS, usage_breakdown: BREAKDOWN, status: { usage_total: BREAKDOWN.total } };
const EMPTY_REPLIES: Replies = {
  github_watcher: { ...WATCHER, repositories: [] }, list_questions: [], status: { usage_total: { ...BREAKDOWN.total, cost_usd: 0, input_tokens: 0, output_tokens: 0 } },
  usage_breakdown: { rows: [], total: { ...BREAKDOWN.total, cost_usd: 0, input_tokens: 0, output_tokens: 0 }, truncated: false },
};

let h: Harness | null = null;
beforeEach(() => { vi.useFakeTimers({ now: NOW, toFake: ["Date", "setInterval", "clearInterval"] }); });
afterEach(async () => {
  await h?.unmount();
  h = null;
  vi.useRealTimers();
});

async function open(connections: readonly ConnectionInfo[], data: boolean, replies: Replies = REPLIES) {
  h = harness(replies);
  seedWorld(h.app.store, connections, data);
  const container = await h.mount(<HomeScreen />);
  await h.frame();
  return { h, container };
}

const tile = (container: HTMLElement, area: string) => container.querySelector<HTMLElement>(`.home-cell[data-area="${area}"]`);
const text = (container: HTMLElement, area: string, part: string) => tile(container, area)?.querySelector(part)?.textContent;
const step = (container: HTMLElement, area: string) => {
  const link = tile(container, area)?.querySelector<HTMLAnchorElement>(".home-step");
  return link ? [link.textContent, link.getAttribute("href")] : null;
};

describe("with nothing paired", () => {
  test("is one guided start with both pairing links and sends nothing", async () => {
    const { h, container } = await open([], false);
    const dashboard = container.querySelector('[data-testid="home-dashboard"]');
    expect(dashboard?.getAttribute("data-mode")).toBe("empty");
    expect(container.querySelector(".ui-state-title")?.textContent).toBe("Connect to Aether");
    expect(container.querySelectorAll(".ui-state-body p")).toHaveLength(2);
    expect([...container.querySelectorAll("a")].map((a) => a.getAttribute("href"))).toEqual(["#/pair/aether", "#/pair/archon"]);
    expect(container.querySelector(".home-grid")).toBeNull();
    expect(h.sent).toEqual([]);
  });
});

describe("while connecting", () => {
  test("shows a skeleton of every tile, busy, and reads nothing yet", async () => {
    const { h, container } = await open([aetherInfo("connecting"), archonInfo("pairing")], false);
    expect(container.querySelector('[data-testid="home-dashboard"]')?.getAttribute("aria-busy")).toBe("true");
    expect(container.querySelectorAll(".home-loading")).toHaveLength(7);
    expect(container.querySelectorAll(".ui-skeleton").length).toBeGreaterThanOrEqual(21);
    expect(h.sent).toEqual([]);
  });
});

describe("connected with nothing yet", () => {
  test("each tile offers its own next step as a quiet link, never a dash", async () => {
    const { container } = await open([aetherInfo(), archonInfo()], false, EMPTY_REPLIES);
    expect(step(container, "agents")).toEqual(["Start an agent →", "#/activity"]);
    expect(step(container, "spend")).toEqual(["Start an agent →", "#/activity"]);
    expect(step(container, "activity")).toEqual(["Start an agent →", "#/activity"]);
    expect(step(container, "queue")).toEqual(["Open the queue →", "#/archon/queue"]);
    expect(step(container, "learn")).toEqual(["Turn on Noesis →", "#/learn"]);
    expect(step(container, "github")).toEqual(["Watch a repository →", "#/github"]);
    expect(text(container, "needs", ".ui-chart-tile-value")).toBe("0");
    expect(text(container, "needs", ".ui-chart-tile-hint")).toBe("Nothing waits on you");
    expect(text(container, "learn", ".ui-chart-tile-value")).toBe("Off");
    expect(container.textContent).not.toMatch(/—|–/);
  });

  test("a daemon not paired points at pairing it, one that is down at the connections", async () => {
    const { container } = await open([aetherInfo(), archonInfo("closed")], false, EMPTY_REPLIES);
    expect(step(container, "queue")).toEqual(["Check connections →", "#/settings/connections"]);
    expect(text(container, "queue", ".ui-chart-tile-value")).toBe("Offline");
    await h?.unmount();
    const only = await open([archonInfo()], false, EMPTY_REPLIES);
    expect(step(only.container, "agents")).toEqual(["Connect to Aether →", "#/pair/aether"]);
    expect(text(only.container, "agents", ".ui-chart-tile-value")).toBe("Not connected");
  });
});

describe("with seeded data", () => {
  test("every tile shows its figure, its line of context and links into its screen", async () => {
    const { container } = await open([aetherInfo(), archonInfo()], true);
    const links = [...container.querySelectorAll<HTMLAnchorElement>(".ui-chart-tile-link")].map((a) => a.getAttribute("href"));
    expect(links).toEqual(["#/inbox", "#/fleet", "#/activity", "#/activity", "#/archon/queue", "#/learn", "#/github"]);
    expect(step(container, "needs"), "the Inbox shows only Aether's needs, so the Archon question gets its own way to the Queue").toEqual(["1 Archon question on the Queue →", "#/archon/queue"]);
    expect(container.querySelectorAll(".home-step")).toHaveLength(1);
    expect(text(container, "agents", ".ui-chart-tile-value")).toBe("2");
    expect(text(container, "agents", ".ui-chart-tile-hint")).toBe("running · 2 waiting · 2 need attention");
    expect(tile(container, "agents")?.querySelectorAll("tbody tr")).toHaveLength(12);
    expect(text(container, "needs", ".ui-chart-tile-value")).toBe("4");
    expect(text(container, "needs", ".ui-chart-tile-hint")).toBe("1 question · 1 approval · 1 stopped agent · 1 Archon question");
    expect(text(container, "spend", ".ui-chart-tile-value")).toBe("$2.25");
    expect(text(container, "spend", ".ui-chart-tile-hint")).toBe("1.4M recorded tokens including cache · partial if subscription turns lack per-message usage");
    expect([...(tile(container, "spend")?.querySelectorAll(".ui-chart-legend li") ?? [])].map((li) => li.textContent))
      .toEqual(["claude-code/opus67%", "claude-code/sonnet27%", "codex/gpt-6-astra7%"]);
  });

  test("with only Archon's questions waiting, Needs you leads to the Queue, where they are answered", async () => {
    const { container } = await open([aetherInfo(), archonInfo()], true, { ...EMPTY_REPLIES, list_questions: QUESTIONS });
    h?.app.store.updateAether(AETHER_CONN, (s) => ({ ...s, attention: { last_seq: 0, questions: [], approvals: [], agents: [] } }));
    await h?.frame();
    expect(text(container, "needs", ".ui-chart-tile-value")).toBe("1");
    expect(text(container, "needs", ".ui-chart-tile-hint")).toBe("1 Archon question");
    expect(tile(container, "needs")?.querySelector(".ui-chart-tile-link")?.getAttribute("href")).toBe("#/archon/queue");
    expect(step(container, "needs")).toBeNull();
  });

  test("the queue, activity, learning and GitHub tiles list what is newest", async () => {
    const { container } = await open([aetherInfo(), archonInfo()], true);
    expect(text(container, "queue", ".ui-chart-tile-value")).toBe("3");
    expect([...(tile(container, "queue")?.querySelectorAll(".home-row") ?? [])].map((row) => row.textContent))
      .toEqual(["Changelog for 1.3Succeeded20 min ago", "Flaky e2e on WindowsFailed1 h ago", "Retire the old APICancelled2 d ago"]);
    expect(tile(container, "queue")?.querySelector('a[href="#/archon/items/qi_done"]')).not.toBeNull();
    const rows = [...(tile(container, "activity")?.querySelectorAll(".home-row") ?? [])];
    expect(rows).toHaveLength(8);
    expect(rows[0]?.textContent).toBe("DaemonInvocation completed1 min ago");
    expect(rows[1]?.querySelector("a")?.getAttribute("href")).toBe(`#/agents/${encodeURIComponent(AETHER_CONN)}/agt_lead`);
    expect(text(container, "activity", ".ui-chart-tile-value")).toBe("3");
    expect(text(container, "learn", ".ui-chart-tile-hint")).toBe("Learning: Rust lifetimes");
    expect(text(container, "learn", ".ui-chart-donut-center")).toBe("8 concepts");
    expect(text(container, "github", ".ui-chart-tile-value")).toBe("2");
    expect(text(container, "github", ".ui-chart-tile-hint")).toBe("2 watched repositories · polled 2 min ago");
    expect(text(container, "github", ".home-problem")).toBe("1 repository failed the last poll");
  });

  test("reads once per connection, not on every change, and again only when a connection comes back", async () => {
    const { h } = await open([aetherInfo(), archonInfo()], true);
    expect([...h.sent].sort()).toEqual([
      `${AETHER_CONN} noesis:learner`, `${AETHER_CONN} noesis:sessions`, `${AETHER_CONN} status`, `${AETHER_CONN} usage_breakdown`,
      `${ARCHON_CONN} github_watcher`, `${ARCHON_CONN} list_questions`,
    ]);
    const [first] = fixtureEvents();
    expect(first).toBeDefined();
    if (first) h.app.store.updateAether(AETHER_CONN, (s) => ({ ...s, events: [...s.events, { ...first, id: "evt_new", seq: 99 }] }));
    await h.frame();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(h.sent).toHaveLength(6);
    await vi.advanceTimersByTimeAsync(HOME_REFRESH_MS);
    await h.frame();
    expect(h.sent.slice(6).sort(), "every read goes again after the refresh period").toEqual([...h.sent.slice(0, 6)].sort());
    h.sent.length = 0;
    h.app.store.update((s) => ({ ...s, connections: { ...s.connections, [ARCHON_CONN]: archonInfo("backoff") } }));
    await h.frame();
    h.app.store.update((s) => ({ ...s, connections: { ...s.connections, [ARCHON_CONN]: archonInfo() } }));
    await h.frame();
    expect(h.sent.sort()).toEqual([`${ARCHON_CONN} github_watcher`, `${ARCHON_CONN} list_questions`]);
  });

  test("shows historical usage even when no agents remain", async () => {
    const { container } = await open([aetherInfo()], false, REPLIES);
    expect(text(container, "spend", ".ui-chart-tile-value")).toBe("$2.25");
    expect(text(container, "spend", ".ui-chart-tile-hint")).toBe("1.4M recorded tokens including cache · partial if subscription turns lack per-message usage");
  });

  test("shows a breakdown failure even when no agents remain", async () => {
    const { container } = await open([aetherInfo()], false, { ...REPLIES, usage_breakdown: new Error("history expired") });
    expect(text(container, "spend", ".ui-chart-tile-value")).toBe("Unknown");
    expect(text(container, "spend", ".home-problem")).toBe("history expired");
  });

  test("labels subscription estimates separately from charges", async () => {
    const total = { ...BREAKDOWN.total, cost_usd: 0, estimated_cost_usd: 4.5, cost_complete: false, cost_basis: "list_price" as const };
    const { container } = await open([aetherInfo()], true, { ...REPLIES, usage_breakdown: { rows: [], total, truncated: false } });
    expect(text(container, "spend", ".ui-chart-tile-value")).toBe("Unknown");
    expect(tile(container, "spend")?.textContent).toContain("$4.50 at API list price for subscription use; not a per-request charge");
    expect(tile(container, "spend")?.textContent).not.toContain("Nothing spent yet today");
  });

  test("incomplete daily usage shows a separate, explicitly lifetime status", async () => {
    const total = { ...BREAKDOWN.total, cost_usd: 0, input_tokens: 0, output_tokens: 0, cost_complete: false };
    const usage = { ...total, input_tokens: 251_000, output_tokens: 159_000, cache_read_tokens: 2_480_000, cache_write_tokens: 796_000, cost_usd: 0.2506 };
    const { container } = await open([aetherInfo()], true, { ...REPLIES, usage_breakdown: { rows: [], total, truncated: false }, status: { usage_total: usage } });
    await h?.frame();
    await h?.frame();
    expect(text(container, "spend", ".ui-chart-tile-value")).toBe("Unknown");
    expect(tile(container, "spend")?.textContent).toContain("partial: subscription turns may lack per-message usage");
    expect(tile(container, "spend")?.textContent).toContain("Lifetime status: 251,000 in · 796,000 cache-write · 2,480,000 cached · 159,000 out · $0.2506+");
  });

  test("an apparently complete zero breakdown stays partial when lifetime status has usage", async () => {
    const zero = { ...BREAKDOWN.total, input_tokens: 0, output_tokens: 0, cost_usd: 0 };
    const { container } = await open([aetherInfo()], false, { ...REPLIES, usage_breakdown: { rows: [], total: zero, truncated: false } });
    await h?.frame();
    expect(text(container, "spend", ".ui-chart-tile-value")).toBe("Unknown");
    expect(tile(container, "spend")?.textContent).toContain("partial: subscription turns may lack per-message usage");
    expect(tile(container, "spend")?.textContent).toContain("Lifetime status:");
  });

  test("lifetime estimates are separate from known cost", async () => {
    const total = { ...BREAKDOWN.total, cost_usd: 0.25, estimated_cost_usd: 0.038, cost_complete: false, cost_basis: "mixed" };
    const breakdown = { rows: [], total, truncated: false };
    const { container } = await open([aetherInfo()], true, { ...REPLIES, usage_breakdown: breakdown, status: { usage_total: total } });
    await h?.frame();
    expect(tile(container, "spend")?.textContent).toContain("$0.2500+ (includes list-price estimates) · $0.0380 subscription API-equivalent estimate");
  });

  test("a failed status read remains visible when the daily breakdown also fails", async () => {
    const { container } = await open([aetherInfo()], true, { ...REPLIES, usage_breakdown: new Error("history expired"), status: new Error("status failed") });
    await h?.frame();
    expect(text(container, "spend", ".ui-chart-tile-value")).toBe("Unknown");
    expect(tile(container, "spend")?.textContent).toContain("history expired");
    expect(tile(container, "spend")?.textContent).toContain("Lifetime status unavailable: status failed");
  });

  test("a stale zero status cannot establish a zero daily total", async () => {
    const zero = { ...BREAKDOWN.total, input_tokens: 0, output_tokens: 0, cost_usd: 0 };
    const { container } = await open([aetherInfo()], false, { ...REPLIES, usage_breakdown: { rows: [], total: zero, truncated: false }, status: { usage_total: zero, stale: { read_at: "2026-11-06T15:19:00Z" } } });
    await h?.frame();
    expect(text(container, "spend", ".ui-chart-tile-value")).toBe("Unknown");
    expect(tile(container, "spend")?.textContent).toContain("Lifetime status (stale)");
  });

  test("legacy daemons do not claim zero today when status has lifetime usage", async () => {
    const { container } = await open([aetherInfo("ready", [])], false, REPLIES);
    await h?.frame();
    expect(text(container, "spend", ".ui-chart-tile-value")).toBe("Unknown");
    expect(tile(container, "spend")?.textContent).toContain("Lifetime status:");
    expect(tile(container, "spend")?.textContent).toContain("partial:");
  });

  test("a stale or unavailable status is never presented as fresh lifetime usage", async () => {
    const total = { ...BREAKDOWN.total, cost_complete: false };
    const breakdown = { rows: [], total, truncated: false };
    const { container } = await open([aetherInfo()], true, { ...REPLIES, usage_breakdown: breakdown, status: { usage_total: total, stale: { read_at: "2026-11-06T15:19:00Z" } } });
    await h?.frame();
    await h?.frame();
    expect(tile(container, "spend")?.textContent).toContain("Lifetime status (stale)");
    await h?.unmount();
    const unavailable = await open([aetherInfo()], true, { ...REPLIES, usage_breakdown: breakdown, status: { usage_total: total, unavailable: [{ section: "usage_total", reason: "timed out" }] } });
    await h?.frame();
    await h?.frame();
    expect(tile(unavailable.container, "spend")?.textContent).toContain("Lifetime status unavailable");
  });

  test("without usage_breakdown, spend today comes from the agents' cost fields and says so", async () => {
    const { h, container } = await open([aetherInfo("ready", ["noesis"]), archonInfo()], true);
    expect(h.sent.some((sent) => sent.endsWith("usage_breakdown"))).toBe(false);
    expect(text(container, "spend", ".ui-chart-tile-value")).toBe("$2.26");
    expect(text(container, "spend", ".ui-chart-tile-hint")).toBe("350k recorded tokens including cache · lifetime cost of agents active today");
  });

  test("a read that fails shows its reason in its own tile and leaves the others alone", async () => {
    const replies = { ...REPLIES, github_watcher: new Error("archond refused github_watcher"), "noesis:learner": new Error("Noesis is busy") };
    const { container } = await open([aetherInfo(), archonInfo()], true, replies);
    await h?.frame();
    expect(text(container, "github", ".ui-chart-tile-value")).toBe("Unknown");
    expect(text(container, "github", ".home-problem")).toBe("archond refused github_watcher");
    expect(text(container, "learn", ".home-problem")).toBe("Mastery could not be read: Noesis is busy");
  });

  test("the GitHub tile points at the overview when archond keeps the watcher for the CLI", async () => {
    const replies = { ...REPLIES, github_watcher: new Error("github_watcher is not available from the web app; use the archon CLI") };
    const { container } = await open([aetherInfo(), archonInfo()], true, replies);
    await h?.frame();
    expect(text(container, "github", ".ui-chart-tile-value")).toBe("CLI");
    expect(text(container, "github", ".ui-chart-tile-hint")).toBe("The watcher reports to the archon CLI");
    expect(container.querySelector('[data-tile="github"] .home-problem')).toBeNull();
    expect(step(container, "github")).toEqual(["See the cached issues and pull requests →", "#/github"]);
    expect(text(container, "spend", ".ui-chart-tile-value")).toBe("$2.25");
  });

  test("explains why the GitHub watcher is unavailable in shared mode", async () => {
    const replies = { ...REPLIES, github_watcher: new Error("invalid argument: the GitHub watcher needs Archon's own Aether; [aether] mode is shared") };
    const { container } = await open([aetherInfo(), archonInfo()], true, replies);
    await h?.frame();
    expect(text(container, "github", ".ui-chart-tile-value")).toBe("Unavailable");
    expect(text(container, "github", ".ui-chart-tile-hint")).toContain("Archon's own Aether");
    expect(container.querySelector('[data-tile="github"] .home-problem')).toBeNull();
  });
});
