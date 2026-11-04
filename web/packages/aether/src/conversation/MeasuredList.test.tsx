// @vitest-environment happy-dom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { describe, expect, test } from "vitest";
import { MeasuredList, rowAt, rowOffsets, rowRange } from "./MeasuredList.tsx";

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
});
