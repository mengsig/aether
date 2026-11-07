import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { toggleCollapsed, visibleGraph } from "./model.ts";
import { GraphNodeList } from "./NodeList.tsx";
import { sceneModel } from "../test/graphs.ts";

const noop = () => undefined;

describe("GraphNodeList", () => {
  it("lists every visible node and expanded group for screen readers", () => {
    const graph = visibleGraph(sceneModel);
    const html = renderToStaticMarkup(
      <GraphNodeList label="Task graph" nodes={graph.nodes} groups={graph.groups} focused="c"
        onFocusNode={noop} onActivate={noop} onToggleGroup={noop} />,
    );
    expect(html).toContain('aria-label="Task graph: 3 nodes"');
    const buttons = [...html.matchAll(/<button([^>]*)>([^<]*)<\/button>/g)].map((m) => m[2]);
    expect(buttons).toEqual(["Group Group", "Alpha, running", "Beta", "Gamma, failed"]);
    expect(html).toMatch(/aria-current="true"[^>]*>Gamma, failed/);
    expect(html).toMatch(/aria-expanded="true"[^>]*>Group Group/);
    expect(html).not.toMatch(/tabindex="0"/);
  });

  it("marks a collapsed group as a collapsed button with its size", () => {
    const graph = visibleGraph(toggleCollapsed(sceneModel, "g"));
    const html = renderToStaticMarkup(
      <GraphNodeList label="Task graph" nodes={graph.nodes} groups={graph.groups} focused={undefined}
        onFocusNode={noop} onActivate={undefined} onToggleGroup={undefined} />,
    );
    expect(html).toContain('aria-label="Task graph: 2 nodes"');
    expect(html).toMatch(/aria-expanded="false"[^>]*>Group, collapsed group of 2 nodes/);
    expect(html).not.toContain("aria-current");
  });
});
