// @vitest-environment happy-dom
import { act } from "react";
import { afterEach, describe, expect, test } from "vitest";
import { render } from "./render.test-util.tsx";
import { VirtualList, visibleRange } from "./virtual-list.tsx";

afterEach(() => {
  document.body.innerHTML = "";
});

function nodeCount(root: Node): number {
  let count = 0;
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_ALL);
  for (let node: Node | null = walker.currentNode; node; node = walker.nextNode()) count += 1;
  return count;
}

async function scrollTo(list: HTMLElement, top: number): Promise<void> {
  await act(async () => {
    list.scrollTop = top;
    list.dispatchEvent(new Event("scroll"));
  });
}

const ROWS = 10_000;
const list = <VirtualList label="Agents" count={ROWS} rowHeight={32} height={480} renderRow={(index) => `agent ${index}`} />;

describe("visibleRange", () => {
  test("covers the viewport plus overscan, clamped to the list", () => {
    expect(visibleRange(0, 480, 32, ROWS, 4)).toEqual({ start: 0, end: 20 });
    expect(visibleRange(32 * 5000, 480, 32, ROWS, 4)).toEqual({ start: 4996, end: 5020 });
    expect(visibleRange(32 * ROWS, 480, 32, ROWS, 4)).toEqual({ start: 9996, end: ROWS });
    expect(visibleRange(0, 480, 32, 0, 4)).toEqual({ start: 0, end: 0 });
  });

  test("refuses impossible geometry", () => {
    expect(() => visibleRange(0, 480, 0, 10, 4)).toThrow(/row height/);
    expect(() => visibleRange(0, 480, 32, 1.5, 4)).toThrow(/whole number/);
  });
});

describe("VirtualList", () => {
  test("renders 10,000 rows with fewer than 60 DOM nodes", async () => {
    const { container } = await render(list);
    const root = container.querySelector<HTMLElement>("[role=list]");
    expect(root).not.toBeNull();
    expect(nodeCount(root as HTMLElement)).toBeLessThan(60);
    const rows = root?.querySelectorAll("[role=listitem]") ?? [];
    expect(rows[0]?.getAttribute("aria-setsize")).toBe(String(ROWS));
    expect((root?.firstElementChild as HTMLElement).style.blockSize).toBe(`${ROWS * 32}px`);
  });

  test("keeps the window small while scrolling to the middle and the end", async () => {
    const { container } = await render(list);
    const root = container.querySelector<HTMLElement>("[role=list]") as HTMLElement;
    await scrollTo(root, 32 * 5000 + 7);
    expect(root.textContent).toContain("agent 5000");
    expect(root.textContent).not.toContain("agent 4990");
    expect(nodeCount(root)).toBeLessThan(60);
    await scrollTo(root, 32 * ROWS);
    const last = [...root.querySelectorAll("[role=listitem]")].pop();
    expect(last?.getAttribute("aria-posinset")).toBe(String(ROWS));
    expect(nodeCount(root)).toBeLessThan(60);
  });

  test("refuses a list taller than browsers can scroll", async () => {
    expect(() => VirtualList({ label: "Huge", count: 1_000_000, rowHeight: 32, renderRow: () => null })).toThrow(/scroll height/);
  });
});
