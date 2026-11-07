import { INITIAL_STATE, type AetherConnState, type ArchonItem } from "@aether-ui/core";
import type { aether, RouteDef } from "@aether-ui/protocol";
import { filterCommands } from "@aether-ui/ui";
import { describe, expect, test, vi } from "vitest";
import { entityCommands, routeOffered, sameSources, selectPaletteSources, type PaletteSources } from "./palette-entities.ts";

const agent = (id: string, name: string, role: string, state = "running"): aether.Agent => ({ id, name, role, status: { state }, archived: false }) as unknown as aether.Agent;

function item(title: string, state: string, extra: Record<string, unknown> = {}): ArchonItem {
  return { summary: { title, pipeline: "build", repo: "acme/site", ...extra }, state, stage: "review", asOf: 1 };
}

const agents = Object.fromEntries(Array.from({ length: 30 }, (_, n) => {
  const named = n === 17 ? agent("ag_17", "Audit Notifications", "auditor") : agent(`ag_${n}`, `Worker ${n}`, "builder");
  return [named.id, named];
}));

const sources: PaletteSources = {
  connections: { local: { id: "local", daemon: "aether", state: "ready", features: [], label: "This machine" }, lab: { id: "lab", daemon: "aether", state: "ready", features: [], label: "Lab box" } },
  agents: { local: agents, lab: { ag_lab: agent("ag_lab", "Lab runner", "builder", "waiting") } },
  sessions: { local: { s1: { id: "s1", agent: "ag_3", goal: "Learn Rust lifetimes", mode: "teach", phase: "plan" } as unknown as aether.NoesisSession } },
  items: {
    queue: {
      qi_1: item("Dark mode", "running", { pr: { host: "github.com", repo: "acme/site", number: 57, kind: "pull_request", state: "draft" } }),
      qi_2: item("Dark mode follow-up", "queued", { pr: { host: "github.com", repo: "acme/site", number: 57, kind: "pull_request" } }),
    },
  },
  initiatives: { queue: [{ id: "in_a", title: "Launch week", state: "open", contract: "", created_at: "", updated_at: "" }] },
};

const hrefs: string[] = [];
const target = { navigate: (href: string) => hrefs.push(href), offered: () => true };
const commands = entityCommands(sources, target);
const find = (query: string) => filterCommands(commands, query);

describe("entity commands", () => {
  test("an agent is found by exact name, partial name, role and id, across connections", () => {
    expect(find("Audit Notifications").map((c) => c.id)).toEqual(["agent:local:ag_17"]);
    expect(find("notif").map((c) => c.id)).toEqual(["agent:local:ag_17"]);
    expect(find("auditor").map((c) => c.id)).toEqual(["agent:local:ag_17"]);
    expect(find("ag_17").map((c) => c.id)).toEqual(["agent:local:ag_17"]);
    expect(find("lab runner")[0]?.subtitle).toBe("builder · waiting · Lab box");
  });

  test("each entity has a kind group, a status subtitle and its own route", () => {
    const byKind = Object.fromEntries(commands.map((c) => [c.group, c]));
    expect(Object.keys(byKind).sort()).toEqual(["Agent", "Initiative", "Item", "Learn session", "Pull request"]);
    for (const [query, group, subtitle, href] of [
      ["audit notif", "Agent", "auditor · running · This machine", "#/agents/local/ag_17"],
      ["dark mode follow", "Item", "queued · review · acme/site", "#/archon/items/qi_2"],
      ["launch", "Initiative", "open", "#/archon/initiatives/in_a"],
      ["acme/site#57", "Pull request", "draft · Dark mode", "#/github/github.com/acme/site/57"],
      ["lifetimes", "Learn session", "teach · plan", "#/learn/sessions/s1"],
    ] as const) {
      const [found] = find(query);
      expect([found?.group, found?.subtitle]).toEqual([group, subtitle]);
      found?.run();
      expect(hrefs.at(-1)).toBe(href);
    }
  });

  test("a pull request carried by two items appears once, and an item is also found by its pull request", () => {
    expect(commands.filter((c) => c.group === "Pull request")).toHaveLength(1);
    expect(find("57").map((c) => c.group)).toContain("Pull request");
  });

  test("an entity whose screen is not offered is left out", () => {
    const withoutLearn = entityCommands(sources, { navigate: vi.fn(), offered: (pattern) => !pattern.startsWith("#/learn") });
    expect(withoutLearn.some((c) => c.group === "Learn session")).toBe(false);
    expect(withoutLearn.some((c) => c.group === "Agent")).toBe(true);
  });

  test("an item without a title is a programmer error, not an empty label", () => {
    const broken = { ...sources, items: { queue: { qi_x: { summary: {}, state: "queued", asOf: null } } } } as PaletteSources;
    expect(() => entityCommands(broken, target)).toThrow(/has a title/);
  });

  test("builds 5,000 entities that stay unique and filter quickly", () => {
    const items = Object.fromEntries(Array.from({ length: 5000 }, (_, n) => [`qi_${n}`, item(`Task ${n}`, "queued")]));
    const big = entityCommands({ ...sources, items: { queue: items } }, target);
    const started = performance.now();
    const found = filterCommands(big, "task 4999");
    expect(performance.now() - started).toBeLessThan(250);
    expect(found[0]?.id).toBe("item:queue:qi_4999");
  });
});

describe("sources", () => {
  test("a store change elsewhere keeps the same sources; a change to agents does not", () => {
    const before = selectPaletteSources(INITIAL_STATE);
    expect(sameSources(before, selectPaletteSources({ ...INITIAL_STATE, reports: [{ id: 1, message: "x" }] }))).toBe(true);
    const local = { agents, learn: { sessions: {} } } as unknown as AetherConnState;
    const withAgents = selectPaletteSources({ ...INITIAL_STATE, aether: { local } });
    expect(sameSources(before, withAgents)).toBe(false);
    expect(sameSources(withAgents, selectPaletteSources({ ...INITIAL_STATE, aether: { local: { ...local, events: [] } } }))).toBe(true);
  });

  test("routeOffered needs a registered route whose requirement holds", () => {
    const route = (path: string, requires?: RouteDef["requires"]): RouteDef => ({ path, owner: "t", load: async () => ({ default: () => null }), ...(requires ? { requires } : {}) });
    const routes = [route("#/a"), route("#/b", (caps) => caps["x"] === true)];
    expect(routeOffered(routes, {}, "#/a")).toBe(true);
    expect(routeOffered(routes, {}, "#/b")).toBe(false);
    expect(routeOffered(routes, { x: true }, "#/b")).toBe(true);
    expect(routeOffered(routes, {}, "#/c")).toBe(false);
  });
});
