import { describe, expect, it } from "vitest";
import { sceneLayout, sceneModel } from "../test/graphs.ts";
import { visibleGraph } from "./model.ts";
import { buildScene } from "./scene.ts";
import { defaultNodeStyle, nodeScale, toneToken } from "./style.ts";

describe("node identity", () => {
  it.each([
    ["agent", "pill"], ["item", "rounded-rect"], ["stage", "diamond"], ["issue", "circle"],
    ["pull_request", "pill"], ["repo", "rounded-rect"], ["proposition", "circle"], ["statement", "pill"],
    ["initiative:one", "rounded-rect"], ["unknown", "rounded-rect"],
  ])("maps %s to %s", (group, shape) => {
    expect(defaultNodeStyle({ id: "n", label: "Node", group }).shape).toBe(shape);
  });

  it("resolves semantic tones exclusively to token names", () => {
    expect(["running", "accent", "muted", "pr-merged", "issue-open", "mastery-fragile"].map(toneToken))
      .toEqual(["--status-running", "--color-accent", "--color-text-muted", "--pr-merged", "--issue-open", "--mastery-fragile"]);
    expect(() => toneToken("#fff")).toThrow();
    expect(() => toneToken("")).toThrow();
  });

  it("uses issue and pull request status families", () => {
    expect(defaultNodeStyle({ id: "p", label: "Pull request", group: "pull_request", status: "merged" }).tone).toBe("pr-merged");
    expect(defaultNodeStyle({ id: "i", label: "Issue", group: "issue", status: "open" }).tone).toBe("issue-open");
  });

  it("carries weights through projection and scales within bounds", () => {
    const model = { ...sceneModel, nodes: sceneModel.nodes.map((n, i) => ({ ...n, weight: i === 0 ? 0 : 100 })) };
    const scene = buildScene(visibleGraph(model), sceneLayout, "box", defaultNodeStyle);
    expect(scene.nodes[0]?.width).toBe(168 * 0.85);
    expect(scene.nodes[1]?.width).toBe(168 * 1.15);
    expect(nodeScale(undefined)).toBe(1);
    expect(() => nodeScale(NaN)).toThrow();
    expect(() => nodeScale(-1)).toThrow();
  });

  it("keeps the old scene contract and geometry when no styling is supplied", () => {
    const scene = buildScene(visibleGraph(sceneModel), sceneLayout, "box");
    expect(scene.nodes.map(({ node, ...box }) => ({ id: node.id, ...box }))).toMatchInlineSnapshot(`
      [
        {
          "height": 44,
          "id": "a",
          "width": 168,
          "x": 16,
          "y": 78,
        },
        {
          "height": 44,
          "id": "b",
          "width": 168,
          "x": 16,
          "y": 178,
        },
        {
          "height": 44,
          "id": "c",
          "width": 168,
          "x": 316,
          "y": 128,
        },
      ]
    `);
  });
});
