// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { Markdown } from "./markdown.tsx";
import { diagramRefusal } from "./mermaid-policy.ts";

const api = vi.hoisted(() => ({ initialize: vi.fn(), render: vi.fn() }));
vi.mock("mermaid", () => ({ default: api }));
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const roots: Root[] = [];
beforeEach(() => {
  document.documentElement.style.cssText = "--color-surface-1: #fff; --color-text: #111; --color-border-strong: #777; --color-accent: #070; --font-ui: sans-serif";
  vi.spyOn(globalThis, "getComputedStyle").mockReturnValue(document.documentElement.style);
});
afterEach(async () => {
  await act(async () => { for (const root of roots.splice(0)) root.unmount(); });
  document.body.replaceChildren();
  vi.clearAllMocks();
  vi.restoreAllMocks();
});

async function render(source: string): Promise<HTMLElement> {
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  roots.push(root);
  await act(async () => { root.render(<Markdown text={`\`\`\`mermaid\n${source}\n\`\`\``} />); });
  await act(async () => { await import("./mermaid-render.ts"); });
  return container;
}

describe("Mermaid image sandbox", () => {
  test("renders a diagram as an inert local SVG image with strict settings", async () => {
    const bindFunctions = vi.fn();
    api.render.mockResolvedValue({ svg: '<svg xmlns="http://www.w3.org/2000/svg" width="100%" viewBox="0 0 420 160"><text>A → B</text></svg>', bindFunctions });
    const root = await render("graph TD\n A --> B");
    expect(root.querySelector("img")?.src).toMatch(/^data:image\/svg\+xml;charset=utf-8,/);
    const svg = decodeURIComponent(root.querySelector("img")?.src.split(",")[1] ?? "");
    expect(svg).toContain('width="420"');
    expect(svg).toContain('height="160"');
    expect(root.querySelector("svg,script,iframe")).toBeNull();
    expect(api.initialize).toHaveBeenCalledWith(expect.objectContaining({ securityLevel: "strict", htmlLabels: false, startOnLoad: false, maxEdges: 200 }));
    expect(bindFunctions).not.toHaveBeenCalled();
    expect(root.querySelector("details code")?.textContent).toContain("A --> B");
    expect(document.querySelector(".md-diagram-measure")).toBeNull();
  });

  test("parse failures show the complete source and a visible reason", async () => {
    api.render.mockRejectedValue(new Error("invalid diagram"));
    const root = await render("not a diagram");
    expect(root.querySelector("img")).toBeNull();
    expect(root.querySelector("code")?.textContent).toBe("not a diagram");
    expect(root.querySelector("figcaption")?.textContent).toContain("invalid diagram");
    expect(document.querySelector(".md-diagram-measure")).toBeNull();
  });

  test.each([
    'graph TD; A-->B; click A "javascript:alert(1)"', "graph TD; A-->B; click A callback",
    'graph TD; A["<script>alert(1)</script>"]', 'graph TD; A["<img src=x onerror=alert(1)>"]',
    'graph TD; A["&lt;img src=x onerror=alert(1)&gt;"]',
    '%%{init: {"securityLevel":"loose"}}%%\ngraph TD; A-->B',
    '---\nconfig:\n  securityLevel: loose\n---\ngraph TD; A-->B',
    'graph TD; A@{ img: "https://evil.test/image.png" }',
    'graph TD; A-->B; style A fill:url(//evil.test/image.svg)',
  ])("rejects hostile source before Mermaid sees it: %s", async (source) => {
    const root = await render(source);
    expect(api.render).not.toHaveBeenCalled();
    expect(root.querySelector("img,svg,script,a,iframe")).toBeNull();
    expect(root.querySelector("code")?.textContent).toBe(source);
    expect(root.querySelector("figcaption")?.textContent).toMatch(/disabled|theme/);
  });

  test("limits source size and leaves safe diagram syntax available", () => {
    expect(diagramRefusal("x".repeat(20_001))).toContain("limit");
    expect(diagramRefusal("graph LR\nA <--> B")).toBeNull();
    expect(diagramRefusal("sequenceDiagram\nAlice->>Bob: Hello")).toBeNull();
    expect(diagramRefusal("graph LR; A[Click to continue] --> B[Callback completed]")).toBeNull();
  });
});
