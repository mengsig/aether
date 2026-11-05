// @vitest-environment happy-dom
import { Registry } from "@aether-ui/core";
import type { Capabilities, EntityRef } from "@aether-ui/protocol";
import { actionsFor } from "@aether-ui/ui";
import { afterEach, describe, expect, test } from "vitest";
import { registeredModules } from "../index.ts";
import routes, { actions } from "./routes.ts";

const ON: Capabilities = { "noesis.available": true, "noesis.engineering_context": true };
const OFF: Capabilities = { "noesis.available": true, "noesis.engineering_context": false };

function registered(): Registry {
  const registry = new Registry();
  for (const action of actions) registry.registerEntityAction(action);
  return registry;
}

afterEach(() => { location.hash = ""; });

describe("Learn from this", () => {
  test("adds no route and is auto-registered by the package", () => {
    expect(routes).toEqual([]);
    expect(registeredModules).toContain("./understand/routes.ts");
  });

  test("is registered for items, runs, agents and pull requests", () => {
    const registry = registered();
    expect(registry.actions().map((action) => [action.kind, action.label])).toEqual([
      ["item", "Learn from this"], ["run", "Learn from this"], ["agent", "Learn from this"], ["pull_request", "Learn from this"],
    ]);
  });

  test("is offered only while engineering context is on", () => {
    const registry = registered();
    const pr: EntityRef = { kind: "pull_request", conn: "archon:local", id: "github.com/acme/forge#212" };
    expect(actionsFor(registry.actions(), ON, pr).map((action) => action.id)).toEqual(["f6.learn-from-pull_request"]);
    expect(actionsFor(registry.actions(), OFF, pr)).toEqual([]);
    expect(actionsFor(registry.actions(), {}, { kind: "item", conn: "archon:local", id: "qi_x" })).toEqual([]);
  });

  test("opens #/learn with the entity in the query", async () => {
    const action = actions.find((entry) => entry.kind === "pull_request");
    await action?.run({ kind: "pull_request", conn: "archon:local", id: "github.com/acme/forge#212" });
    expect(location.hash).toBe("#/learn?understand=pull_request&conn=archon%3Alocal&id=github.com%2Facme%2Fforge%23212");
  });
});
