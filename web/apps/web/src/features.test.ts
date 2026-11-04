import { registry } from "@aether-ui/core";
import { describe, expect, test } from "vitest";
import * as aether from "@aether-ui/aether";
import * as archon from "@aether-ui/archon";
import * as github from "@aether-ui/github";
import * as noesis from "@aether-ui/noesis";
import * as settings from "@aether-ui/settings";

describe("feature packages", () => {
  test("each loads its routes.ts files by glob and registers their routes", () => {
    const packages = { aether, archon, github, noesis, settings };
    const loaded: Record<string, readonly string[]> = {
      aether: ["./agent/routes.ts", "./fleet/routes.ts", "./topology/routes.ts"], archon: ["./item/routes.ts", "./pipeline/routes.ts", "./queue/routes.ts"],
    };
    for (const [name, feature] of Object.entries(packages)) expect(feature.registeredModules, name).toEqual(loaded[name] ?? []);
    expect(registry.routes().map((route) => [route.path, route.owner])).toEqual([
      ["#/agents/:conn/:agent", "B2"], ["#/fleet", "B1"], ["#/fleet/topology", "B1"],
      ["#/archon/items/:item", "C1"], ["#/archon/items/:item/runs/:run", "C1"], ["#/archon/queue", "C1"],
    ]);
    expect(registry.actions()).toEqual([]);
  });
});
