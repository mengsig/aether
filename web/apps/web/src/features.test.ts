import { registry } from "@aether-ui/core";
import { describe, expect, test } from "vitest";
import * as aether from "@aether-ui/aether";
import * as archon from "@aether-ui/archon";
import * as github from "@aether-ui/github";
import * as noesis from "@aether-ui/noesis";
import * as settings from "@aether-ui/settings";

describe("feature package skeletons", () => {
  test("each loads its routes.ts files by glob and none has any yet", () => {
    const packages = { aether, archon, github, noesis, settings };
    for (const [name, feature] of Object.entries(packages)) expect(feature.registeredModules, name).toEqual([]);
    expect(registry.routes()).toEqual([]);
    expect(registry.actions()).toEqual([]);
  });
});
