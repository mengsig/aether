import { registry } from "@aether-ui/core";
import { describe, expect, test } from "vitest";
import * as aether from "@aether-ui/aether";
import * as archon from "@aether-ui/archon";
import * as github from "@aether-ui/github";
import * as noesis from "@aether-ui/noesis";
import * as settings from "@aether-ui/settings";
import { appRoutes } from "./routes.ts";
import { screenTitles } from "./titles.ts";

describe("feature packages", () => {
  test("each loads its routes.ts files by glob and registers their routes", () => {
    const packages = { aether, archon, github, noesis, settings };
    const loaded: Record<string, readonly string[]> = {
      aether: ["./activity/routes.ts", "./agent/routes.ts", "./controls/routes.ts", "./fleet/routes.ts", "./inbox/routes.ts", "./memory/routes.ts", "./topology/routes.ts"],
      archon: ["./dag/routes.ts", "./history/routes.ts", "./initiatives/routes.ts", "./item/routes.ts", "./pipeline/routes.ts", "./queue/routes.ts"],
      github: ["./issue/routes.ts", "./overview/routes.ts"],
      noesis: ["./learn/routes.ts", "./understand/routes.ts"],
      settings: ["./screens/routes.ts"],
    };
    for (const [name, feature] of Object.entries(packages)) expect(feature.registeredModules, name).toEqual(loaded[name] ?? []);
    expect(registry.routes().map((route) => [route.path, route.owner])).toEqual([
      ["#/activity", "B3"], ["#/agents/:conn/:agent", "B2"], ["#/controls/:conn/:agent", "B3"], ["#/fleet", "B1"], ["#/inbox", "B3"],
      ["#/memory/:conn", "B3"], ["#/fleet/topology", "B1"],
      ["#/archon/graph", "C2"], ["#/archon/history", "C2"], ["#/archon/initiatives/:id", "C2"],
      ["#/archon/items/:item", "C1"], ["#/archon/items/:item/runs/:run", "C1"], ["#/archon/queue", "C1"],
      ["#/github/:host/:owner/:repo/:number", "D4"], ["#/github", "D4"],
      ["#/learn", "F5"], ["#/learn/sessions/:id", "F5"], ["#/learn/graph", "F5"], ["#/learn/me", "F5"],
      ["#/settings/*", "B3"],
    ]);
    expect(registry.actions().map((action) => [action.id, action.kind])).toEqual([
      ["b3.agent-controls", "agent"], ["d4.open-issue", "issue"], ["d4.open-pull-request", "pull_request"], ["d4.import-issue", "issue"],
      ["f6.learn-from-item", "item"], ["f6.learn-from-run", "run"], ["f6.learn-from-agent", "agent"], ["f6.learn-from-pull_request", "pull_request"],
    ]);
  });

  test("every route without a nav entry has a header title, and every title names a route", () => {
    const routes = [...registry.routes(), ...appRoutes];
    const navless = routes.filter((route) => route.nav === undefined).map((route) => route.path);
    expect(navless.length).toBeGreaterThan(0);
    expect(navless.filter((path) => screenTitles[path] === undefined)).toEqual([]);
    expect(Object.keys(screenTitles).filter((path) => !navless.includes(path))).toEqual([]);
  });
});
