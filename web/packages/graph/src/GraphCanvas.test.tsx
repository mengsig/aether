// @vitest-environment happy-dom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { sceneLayout, sceneModel } from "../test/graphs.ts";
import { GraphCanvas } from "./GraphCanvas.tsx";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// happy-dom does not inherit custom properties, so the rules target the canvas itself.
const TOKENS = `
  [data-theme="light"] canvas { --color-surface-1: #fbfaf6; --color-text: #10302a; }
  [data-theme="dark"] canvas { --color-surface-1: #1b2422; --color-text: #e8e4d8; }
`;

/** Every fill colour the canvas used, in order. */
function recordFills(): string[] {
  const fills: string[] = [];
  const ctx = {
    fillStyle: "", strokeStyle: "", lineWidth: 1, font: "", textBaseline: "alphabetic",
    setTransform: () => undefined, clearRect: () => undefined, strokeRect: () => undefined, beginPath: () => undefined,
    moveTo: () => undefined, lineTo: () => undefined, bezierCurveTo: () => undefined, stroke: () => undefined,
    fillRect(this: { fillStyle: string }) { fills.push(this.fillStyle); },
    fillText: () => undefined,
    measureText: (text: string) => ({ width: text.length * 7 }),
  };
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(ctx as unknown as CanvasRenderingContext2D);
  return fills;
}

/** Lets pending observers and animation frames run. */
const settle = () => act(async () => { await new Promise((resolve) => setTimeout(resolve, 40)); });

beforeEach(() => {
  vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(800);
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(400);
  vi.stubGlobal("ResizeObserver", class {
    readonly callback: () => void;
    constructor(callback: () => void) { this.callback = callback; }
    observe() { this.callback(); }
    disconnect() { return undefined; }
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
  document.head.innerHTML = "";
});

describe("GraphCanvas", () => {
  it("redraws in the new theme's tokens when data-theme changes while mounted", async () => {
    const style = document.createElement("style");
    style.textContent = TOKENS;
    document.head.append(style);
    const themed = document.createElement("div");
    themed.dataset.theme = "light";
    document.body.append(themed);
    const fills = recordFills();
    const root = createRoot(themed);
    await act(async () => root.render(<GraphCanvas model={sceneModel} layout={sceneLayout} label="Tasks" />));
    await settle();
    expect(fills).toContain("#fbfaf6");
    expect(fills).not.toContain("#1b2422");

    // happy-dom refreshes computed styles only one level down, so switch the canvas's parent.
    const frame = themed.querySelector("canvas")?.parentElement;
    if (frame === null || frame === undefined) throw new Error("GraphCanvas renders a canvas in a frame");
    fills.length = 0;
    frame.dataset.theme = "dark";
    await settle();
    expect(fills).toContain("#1b2422");
    expect(fills).not.toContain("#fbfaf6");

    await act(async () => root.unmount());
  });

  it("resizes the backing store when only the pixel ratio changes, and stops watching on unmount", async () => {
    const queries: Array<{ media: string; listeners: Set<() => void> }> = [];
    vi.stubGlobal("matchMedia", (media: string) => {
      const query = { media, listeners: new Set<() => void>() };
      queries.push(query);
      return {
        addEventListener: (_: string, listener: () => void) => query.listeners.add(listener),
        removeEventListener: (_: string, listener: () => void) => query.listeners.delete(listener),
      };
    });
    vi.stubGlobal("devicePixelRatio", 1);
    recordFills();
    const host = document.createElement("div");
    document.body.append(host);
    const root = createRoot(host);
    await act(async () => root.render(<GraphCanvas model={sceneModel} layout={sceneLayout} label="Tasks" />));
    const canvas = host.querySelector("canvas");
    if (canvas === null) throw new Error("GraphCanvas renders a canvas");
    expect([canvas.width, canvas.height]).toEqual([800, 400]);

    vi.stubGlobal("devicePixelRatio", 2);
    await act(async () => { for (const listener of [...(queries.at(-1)?.listeners ?? [])]) listener(); });
    expect([canvas.width, canvas.height]).toEqual([1600, 800]);
    expect(queries.map((q) => q.media)).toEqual(["(resolution: 1dppx)", "(resolution: 2dppx)"]);
    expect(queries[0]?.listeners.size).toBe(0);

    await act(async () => root.unmount());
    expect(queries.every((q) => q.listeners.size === 0)).toBe(true);
  });
});
