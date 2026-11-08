// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { sceneLayout, sceneModel } from "../test/graphs.ts";
import { GraphCanvas, type GraphCanvasProps } from "./GraphCanvas.tsx";
import { GraphRenderer } from "./renderer.ts";
import { Surface } from "./surface.ts";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
let root: Root | undefined;

beforeEach(() => {
  const noop = () => undefined;
  const ctx = { setTransform: noop, clearRect: noop, fillRect: noop, strokeRect: noop, beginPath: noop, moveTo: noop,
    lineTo: noop, bezierCurveTo: noop, stroke: noop, fill: noop, fillText: noop, strokeText: noop, setLineDash: noop,
    measureText: (text: string) => ({ width: text.length * 7 }) };
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(ctx as unknown as CanvasRenderingContext2D);
  vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(800);
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(400);
  vi.stubGlobal("ResizeObserver", class {
    private readonly callback: () => void;
    constructor(callback: () => void) { this.callback = callback; }
    observe() { this.callback(); }
    disconnect() { return undefined; }
  });
});

afterEach(async () => {
  await act(async () => root?.unmount());
  root = undefined;
  vi.restoreAllMocks(); vi.unstubAllGlobals();
  document.body.innerHTML = "";
  document.head.innerHTML = "";
});

async function mount(props: Partial<GraphCanvasProps> = {}) {
  const host = document.createElement("div"); document.body.append(host);
  root = createRoot(host);
  await act(async () => root?.render(<GraphCanvas model={sceneModel} layout={sceneLayout} label="Tasks" {...props} />));
  const canvas = host.querySelector("canvas");
  if (canvas === null) throw new Error("the graph mounts a canvas");
  return { host, canvas };
}

async function click(host: HTMLElement, testid: string) {
  const button = host.querySelector<HTMLButtonElement>(`[data-testid='${testid}']`);
  if (button === null) throw new Error(`missing graph control: ${testid}`);
  await act(async () => button.click());
}

const key = (canvas: HTMLCanvasElement, value: string) => act(async () => { canvas.dispatchEvent(new KeyboardEvent("keydown", { key: value, bubbles: true })); });

describe("graph interaction", () => {
  it("offers labelled zoom and fit buttons that change the camera", async () => {
    const moved = vi.spyOn(GraphRenderer.prototype, "moveCamera");
    const { host } = await mount();
    const initial = moved.mock.calls.at(-1)?.[0];
    await click(host, "graph-zoom-in");
    expect(moved.mock.calls.at(-1)?.[0].scale).toBe((initial?.scale ?? 0) * 1.25);
    await click(host, "graph-zoom-out");
    expect(moved.mock.calls.at(-1)?.[0].scale).toBe(initial?.scale);
    await click(host, "graph-fit");
    expect(moved.mock.calls.at(-1)?.[0]).toEqual(initial);
    expect(host.querySelector("[data-testid=graph-fit]")?.getAttribute("aria-label")).toBe("Fit graph");
  });

  it("keeps +, -, 0, wheel and double-click zoom available with controls hidden", async () => {
    const moved = vi.spyOn(GraphRenderer.prototype, "moveCamera");
    const { host, canvas } = await mount({ controls: false });
    expect(host.querySelector("[data-testid=graph-fit]")).toBeNull();
    const initial = moved.mock.calls.at(-1)?.[0];
    await key(canvas, "+");
    expect(moved.mock.calls.at(-1)?.[0].scale).toBe((initial?.scale ?? 0) * 1.25);
    await key(canvas, "-");
    expect(moved.mock.calls.at(-1)?.[0].scale).toBe(initial?.scale);
    await act(async () => {
      const wheel = new WheelEvent("wheel", { deltaY: -100, cancelable: true });
      Object.defineProperties(wheel, { clientX: { value: 120 }, clientY: { value: 80 } });
      canvas.dispatchEvent(wheel);
    });
    expect(moved.mock.calls.at(-1)?.[0].scale).toBeGreaterThan(initial?.scale ?? 0);
    await key(canvas, "0");
    expect(moved.mock.calls.at(-1)?.[0]).toEqual(initial);
    await act(async () => { canvas.dispatchEvent(new MouseEvent("dblclick", { clientX: 120, clientY: 80 })); });
    expect(moved.mock.calls.at(-1)?.[0].scale).toBe((initial?.scale ?? 0) * 1.5);
  });

  it("renders a semantic legend and consumer-owned empty content", async () => {
    const { host } = await mount({ legend: [{ label: "Running", status: "running", shape: "pill" }, { label: "Dependency", tone: "muted", edge: "dashed" }] });
    const legend = host.querySelector("[data-testid=graph-legend]");
    expect(legend?.tagName).toBe("UL");
    expect(legend?.querySelectorAll("li").length).toBe(2);
    expect(legend?.textContent).toContain("Dependency");
    await act(async () => root?.render(<GraphCanvas model={{ nodes: [], edges: [], groups: [], collapsed: new Set() }} layout={undefined}
      label="Tasks" emptyState={<a href="#/queue">Queue an item</a>} />));
    expect(host.querySelector("[data-testid=graph-empty] a")?.textContent).toBe("Queue an item");
    expect(host.querySelector("[data-testid=graph-fit]")).toBeNull();
  });

  it("announces focused nodes, calls details and preserves activation callbacks", async () => {
    const onSelect = vi.fn(), onActivate = vi.fn();
    const details = vi.fn((node: { label: string }) => <strong>{node.label} details</strong>);
    const { host, canvas } = await mount({ onSelect, onActivate, details });
    await key(canvas, "Home");
    expect(onSelect).toHaveBeenLastCalledWith(expect.objectContaining({ id: "a" }));
    expect(host.querySelector("[data-testid=graph-details]")?.textContent).toBe("Alpha details");
    expect(host.querySelector("[aria-live=polite]")?.textContent).toContain("Alpha");
    await key(canvas, "Enter");
    expect(onActivate).toHaveBeenCalledWith("a");
    await key(canvas, "Escape"); await key(canvas, "Escape");
    expect(onSelect).toHaveBeenLastCalledWith(null);
    expect(host.querySelector("[data-testid=graph-details]")).toBeNull();
  });

  it("exposes full hover labels and clears hover when the pointer leaves", async () => {
    const onHover = vi.fn();
    vi.spyOn(Surface.prototype, "hitAt").mockReturnValue({ kind: "node", id: "a" });
    const { host, canvas } = await mount({ onHover });
    await act(async () => { canvas.dispatchEvent(new PointerEvent("pointermove", { clientX: 100, clientY: 100 })); });
    expect(onHover).toHaveBeenLastCalledWith(expect.objectContaining({ id: "a", label: "Alpha" }));
    expect(host.querySelector("[role=tooltip]")?.textContent).toBe("Alpha");
    await act(async () => { canvas.dispatchEvent(new PointerEvent("pointerleave")); });
    expect(onHover).toHaveBeenLastCalledWith(null);
    expect(host.querySelector("[role=tooltip]")).toBeNull();
  });

  it("pinches around two pointers and keeps panning after one lifts", async () => {
    const moved = vi.spyOn(GraphRenderer.prototype, "moveCamera"), onActivate = vi.fn();
    const { canvas } = await mount({ onActivate });
    canvas.setPointerCapture = vi.fn();
    const initial = moved.mock.calls.at(-1)?.[0];
    const pointer = (type: string, id: number, x: number) => act(async () => {
      canvas.dispatchEvent(new PointerEvent(type, { pointerId: id, clientX: x, clientY: 100, button: 0 }));
    });
    await pointer("pointerdown", 1, 100);
    await pointer("pointerdown", 2, 200);
    await pointer("pointermove", 2, 300);
    expect(moved.mock.calls.at(-1)?.[0].scale).toBe((initial?.scale ?? 0) * 2);
    await pointer("pointerup", 1, 100);
    await pointer("lostpointercapture", 1, 100);
    const before = moved.mock.calls.at(-1)?.[0];
    await pointer("pointermove", 2, 310);
    expect(moved.mock.calls.at(-1)?.[0].x).toBe((before?.x ?? 0) + 10);
    await pointer("pointerup", 2, 310);
    expect(onActivate).not.toHaveBeenCalled();
  });

  it("uses immediate layout and camera updates under reduced motion", async () => {
    const style = document.createElement("style");
    style.textContent = "canvas { --motion-slow: 280ms; --ease-standard: cubic-bezier(0.2, 0, 0, 1); }";
    document.head.append(style);
    vi.stubGlobal("matchMedia", (media: string) => ({ matches: media.includes("prefers-reduced-motion"), addEventListener: () => undefined, removeEventListener: () => undefined }));
    const moved = vi.spyOn(GraphRenderer.prototype, "moveCamera");
    const styled = vi.spyOn(GraphRenderer.prototype, "setScene");
    const { host } = await mount();
    await click(host, "graph-zoom-in");
    expect(moved.mock.calls.at(-1)?.[1].duration).toBe(0);
    expect(styled.mock.calls.at(-1)?.[1].duration).toBe(0);
  });
});
