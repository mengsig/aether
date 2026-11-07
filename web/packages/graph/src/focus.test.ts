import { describe, expect, it } from "vitest";
import { announce, describeNode, keyAction, nextFocus } from "./focus.ts";
import { toggleCollapsed, visibleGraph } from "./model.ts";
import { buildScene } from "./scene.ts";
import { sceneLayout, sceneModel } from "../test/graphs.ts";

describe("keyboard focus", () => {
  const scene = buildScene(visibleGraph(sceneModel), sceneLayout, "box");

  it("moves with the arrows to the nearest node that way and stays at an edge", () => {
    expect(nextFocus(scene.nodes, undefined, "ArrowDown")).toBe("a");
    expect(nextFocus(scene.nodes, "a", "ArrowDown")).toBe("b");
    expect(nextFocus(scene.nodes, "a", "ArrowRight")).toBe("c");
    expect(nextFocus(scene.nodes, "c", "ArrowLeft")).toBe("a");
    expect(nextFocus(scene.nodes, "a", "ArrowUp")).toBe("a");
    expect(nextFocus(scene.nodes, "b", "Home")).toBe("a");
    expect(nextFocus(scene.nodes, "a", "End")).toBe("c");
    expect(nextFocus([], undefined, "Home")).toBeUndefined();
  });

  it("activates nodes, toggles groups and climbs out with Escape", () => {
    expect(keyAction(scene, "a", "Enter")).toEqual({ kind: "activate", id: "a" });
    expect(keyAction(scene, "a", "Escape")).toEqual({ kind: "focus", id: "g" });
    expect(keyAction(scene, "g", "Enter")).toEqual({ kind: "toggle", group: "g" });
    expect(keyAction(scene, "g", "Escape")).toEqual({ kind: "blur" });
    expect(keyAction(scene, undefined, "Enter")).toBeUndefined();
    const collapsed = buildScene(visibleGraph(toggleCollapsed(sceneModel, "g")), { hash: "c", positions: new Map([["g", { x: 0, y: 0 }]]) }, "box");
    expect(keyAction(collapsed, "g", " ")).toEqual({ kind: "toggle", group: "g" });
  });

  it("maps zoom and fit keys and ignores others", () => {
    expect(keyAction(scene, undefined, "+")).toEqual({ kind: "zoom", factor: 1.25 });
    expect(keyAction(scene, undefined, "-")).toEqual({ kind: "zoom", factor: 0.8 });
    expect(keyAction(scene, undefined, "0")).toEqual({ kind: "fit" });
    expect(keyAction(scene, undefined, "q")).toBeUndefined();
  });

  it("announces the focused node with its status, and groups with their size", () => {
    expect(announce(scene, "c")).toBe("Gamma, failed");
    expect(announce(scene, "g")).toBe("Group, expanded group");
    expect(announce(scene, undefined)).toBe("");
    expect(describeNode({ id: "g", label: "Group", collapsedMembers: 1 })).toBe("Group, collapsed group of 1 node");
  });
});
