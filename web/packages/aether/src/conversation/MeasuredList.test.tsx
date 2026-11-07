// @vitest-environment happy-dom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, test, vi } from "vitest";
import { firstAtOrAfter, MeasuredList, rowAt, rowOffsets, rowRange } from "./MeasuredList.tsx";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("row geometry", () => {
  const offsets = rowOffsets(4, (index) => [10, 20, 30, 40][index] ?? 0);

  test("offsets accumulate each row's height", () => {
    expect([...offsets]).toEqual([0, 10, 30, 60, 100]);
    expect(() => rowOffsets(2, () => Number.NaN)).toThrow(/real height/);
  });

  test("finds the row under a point and the rows in a band", () => {
    expect([rowAt(offsets, 0), rowAt(offsets, 9), rowAt(offsets, 10), rowAt(offsets, 59), rowAt(offsets, 1_000)]).toEqual([0, 0, 1, 2, 4]);
    expect(rowRange(offsets, 30, 20, 0)).toEqual({ start: 2, end: 3 });
    expect(rowRange(offsets, 30, 20, 15)).toEqual({ start: 1, end: 4 });
    expect(rowRange(rowOffsets(0, () => 0), 0, 600, 800)).toEqual({ start: 0, end: 0 });
  });

  test("finds the first row at or after a position in the shared order", () => {
    const positions = [2, 4, 4, 9];
    const at = (wanted: number) => firstAtOrAfter(positions.length, (i) => positions[i] ?? 0, wanted);
    expect([at(0), at(2), at(3), at(4), at(9), at(10)]).toEqual([0, 0, 1, 1, 3, 4]);
  });
});

describe("a layout change", () => {
  test("keeps the row at the top of the view in view, or the next one a new layout still draws", async () => {
    vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockImplementation(function (this: HTMLElement) {
      return this.classList.contains("conv-list") ? 400 : 0;
    });
    const container = document.createElement("div");
    document.body.append(container);
    const root = createRoot(container);
    const all = Array.from({ length: 1_000 }, (_, i) => i);
    const draw = (layout: string, positions: readonly number[]) => (
      <MeasuredList label="Rows" count={positions.length} rowKey={(i) => `${layout}|${positions[i]}`} estimate={() => 50}
        renderRow={(i) => <span>row {positions[i]}</span>} layout={layout} position={(i) => positions[i] ?? 0} />
    );
    await act(async () => root.render(draw("standard", all)));
    const list = container.querySelector<HTMLElement>(".conv-list");
    if (!list) throw new Error("the list is drawn");
    await act(async () => {
      list.scrollTop = 301 * 50 + 10;
      list.dispatchEvent(new Event("scroll"));
    });
    const odd = all.filter((p) => p % 2 === 1);
    await act(async () => root.render(draw("calm", odd)));
    expect(list.scrollTop, "row 301 is the 151st odd row, 10px in as before").toBe(150 * 50 + 10);
    const even = all.filter((p) => p % 2 === 0);
    await act(async () => root.render(draw("focus", even)));
    expect(list.scrollTop, "row 301 is not drawn; the next one, 302, is at the top").toBe(151 * 50);
    act(() => root.unmount());
    container.remove();
    vi.restoreAllMocks();
  });
});

describe("the measured list", () => {
  test("draws a screenful of 20,000 rows, starting at the end when it follows", async () => {
    const container = document.createElement("div");
    document.body.append(container);
    const root = createRoot(container);
    await act(async () => root.render(
      <MeasuredList label="Rows" count={20_000} rowKey={(i) => `r${i}`} estimate={() => 50} renderRow={(i) => <span>row {i}</span>} follow />,
    ));
    const rows = [...container.querySelectorAll("[role=listitem]")];
    expect(rows.length).toBeLessThan(60);
    expect(rows.at(-1)?.textContent).toBe("row 19999");
    expect(container.querySelector(".conv-spacer")?.getAttribute("style")).toContain("1000000px");
    act(() => root.unmount());
    container.remove();
  });

  describe("when the viewport resizes", () => {
    let height = 400;
    afterEach(() => vi.restoreAllMocks());

    async function mountList(follow: boolean) {
      height = 400;
      vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockImplementation(function (this: HTMLElement) {
        return this.classList.contains("conv-list") ? height : 0;
      });
      vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(() => 1_000 * 50);
      const container = document.createElement("div");
      document.body.append(container);
      const root = createRoot(container);
      await act(async () => root.render(
        <MeasuredList label="Rows" count={1_000} rowKey={(i) => `r${i}`} estimate={() => 50} renderRow={(i) => <span>row {i}</span>} follow={follow} />,
      ));
      const list = container.querySelector<HTMLElement>(".conv-list");
      if (!list) throw new Error("the list is drawn");
      const resize = async (next: number) => {
        height = next;
        await act(async () => window.dispatchEvent(new Event("resize")));
      };
      const lastDrawn = () => Number([...container.querySelectorAll("[role=listitem]")].at(-1)?.getAttribute("aria-posinset"));
      return { list, resize, lastDrawn, unmount: () => { act(() => root.unmount()); container.remove(); } };
    }

    test("draws rows down to the bottom of a viewport that grew", async () => {
      const { list, resize, lastDrawn, unmount } = await mountList(false);
      await act(async () => {
        list.scrollTop = 10_000;
        list.dispatchEvent(new Event("scroll"));
      });
      await resize(1_600);
      expect(lastDrawn() * 50).toBeGreaterThanOrEqual(10_000 + 1_600);
      unmount();
    });

    test("keeps the last row in view while following", async () => {
      const { list, resize, lastDrawn, unmount } = await mountList(true);
      await resize(250);
      expect(list.scrollTop).toBe(50_000 - 250);
      expect(lastDrawn()).toBe(1_000);
      unmount();
    });
  });
});
