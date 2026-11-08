// @vitest-environment happy-dom
import { afterEach, describe, expect, test } from "vitest";
import { BarChart, Donut, donutArcs, Sparkline, sparkPath, StatTile } from "./charts.tsx";
import { render } from "./render.test-util.tsx";

afterEach(() => {
  document.body.innerHTML = "";
});

const named = (svg: SVGElement | null, part: "title" | "desc") => svg?.querySelector(part)?.textContent;

describe("Sparkline", () => {
  test("is a named image with a description and a table of its values", async () => {
    const points = [{ label: "09:00", value: 2 }, { label: "10:00", value: 5 }, { label: "11:00", value: 3 }];
    const { container } = await render(<Sparkline label="Events per hour" points={points} tone="info" />);
    const svg = container.querySelector("svg");
    expect(svg?.getAttribute("role")).toBe("img");
    expect(named(svg, "title")).toBe("Events per hour");
    expect(named(svg, "desc")).toBe("3 values from 09:00 to 11:00; latest 3, highest 5.");
    const ids = svg?.getAttribute("aria-labelledby")?.split(" ") ?? [];
    expect(ids.map((id) => document.getElementById(id)?.tagName.toLowerCase())).toEqual(["title", "desc"]);
    const rows = [...container.querySelectorAll("table tbody tr")].map((row) => row.textContent);
    expect(rows).toEqual(["09:002", "10:005", "11:003"]);
    expect(container.querySelector("figure")?.dataset.tone).toBe("info");
    expect([...container.querySelectorAll(".ui-chart-hit title")].map((t) => t.textContent)).toEqual(["09:00: 2", "10:00: 5", "11:00: 3"]);
  });

  test("puts zero at the bottom and the highest value at the top", () => {
    expect(sparkPath([0, 4, 2], 32)).toBe("M0 30.00 L50 2.00 L100 16.00");
    expect(sparkPath([3], 32)).toBe("M0 2.00 L100 2.00");
    expect(sparkPath([], 32)).toBe("M0 30 L100 30");
    expect(() => sparkPath([1], 4)).toThrow(/taller than its padding/);
  });

  test("says when it has no data and refuses values that are not numbers", async () => {
    const { container } = await render(<Sparkline label="Quiet" points={[]} />);
    expect(named(container.querySelector("svg"), "desc")).toBe("No data yet.");
    expect(() => Sparkline({ label: "Bad", points: [{ label: "a", value: Number.NaN }] })).toThrow(/finite number/);
  });
});

describe("BarChart", () => {
  test("scales columns to the maximum and labels each one", async () => {
    const bars = [{ label: "Succeeded", value: 4, tone: "success" as const }, { label: "Failed", value: 1, tone: "danger" as const }];
    const { container } = await render(<BarChart label="Outcomes" bars={bars} max={8} />);
    const marks = [...container.querySelectorAll<SVGRectElement>(".ui-chart-mark")];
    expect(marks.map((mark) => [mark.getAttribute("height"), mark.dataset.tone])).toEqual([["50", "success"], ["12.5", "danger"]]);
    expect(named(container.querySelector("svg"), "desc")).toBe("Succeeded 4, Failed 1");
    expect(container.querySelector(".ui-chart-axis")?.getAttribute("aria-hidden")).toBe("true");
    expect(container.querySelectorAll("table tbody tr")).toHaveLength(2);
  });

  test("refuses negative bars, bars above the maximum and repeated labels", () => {
    expect(() => BarChart({ label: "x", bars: [{ label: "a", value: -1 }] })).toThrow(/nonnegative/);
    expect(() => BarChart({ label: "x", bars: [{ label: "a", value: 3 }], max: 2 })).toThrow(/above the scale/);
    expect(() => BarChart({ label: "x", bars: [{ label: "a", value: 1 }, { label: "a", value: 2 }] })).toThrow(/unique/);
  });
});

describe("Donut", () => {
  test("goes once round with a gap between slices and skips empty ones", () => {
    const arcs = donutArcs([{ label: "a", value: 3, tone: "accent" }, { label: "b", value: 0, tone: "info" }, { label: "c", value: 1, tone: "neutral" }]);
    expect(arcs).toEqual([{ label: "a", tone: "accent", start: 0, length: 74 }, { label: "c", tone: "neutral", start: 75, length: 24 }]);
    expect(donutArcs([{ label: "a", value: 2, tone: "accent" }])).toEqual([{ label: "a", tone: "accent", start: 0, length: 100 }]);
    expect(donutArcs([])).toEqual([]);
  });

  test("labels every slice with its share, so colour is never the only key", async () => {
    const slices = [{ label: "opus", value: 3, tone: "accent" as const }, { label: "sonnet", value: 1, tone: "warning" as const }];
    const { container } = await render(<Donut label="Spend by model" slices={slices} center="$4.00" />);
    expect([...container.querySelectorAll(".ui-chart-legend li")].map((li) => li.textContent)).toEqual(["opus75%", "sonnet25%"]);
    expect(named(container.querySelector("svg"), "desc")).toBe("$4.00. opus 3 (75%), sonnet 1 (25%)");
    expect(container.querySelectorAll(".ui-chart-slice")).toHaveLength(2);
    expect(container.querySelector(".ui-chart-donut-center")?.textContent).toBe("$4.00");
  });
});

describe("StatTile", () => {
  test("names itself by its heading and links the whole tile through it", async () => {
    const { container } = await render(<StatTile label="Agents" value="12" hint="3 waiting" tone="accent" href="#/fleet"><p>chart</p></StatTile>);
    const tile = container.querySelector("article");
    const heading = tile?.querySelector("h3");
    expect(tile?.getAttribute("aria-labelledby")).toBe(heading?.id);
    expect(heading?.querySelector("a")?.getAttribute("href")).toBe("#/fleet");
    expect(tile?.querySelector(".ui-chart-tile-value")?.textContent).toBe("12");
    expect(tile?.querySelector(".ui-chart-tile-hint")?.textContent).toBe("3 waiting");
    expect(tile?.querySelector(".ui-chart-tile-body")?.textContent).toBe("chart");
  });

  test("without a link it is a plain heading, and it refuses an empty value or a foreign link", async () => {
    const { container } = await render(<StatTile label="Spend" value="$0.00" />);
    expect(container.querySelector("h3")?.textContent).toBe("Spend");
    expect(container.querySelector("a")).toBeNull();
    expect(() => StatTile({ label: "Spend", value: " " })).toThrow(/a label and a value/);
    expect(() => StatTile({ label: "Spend", value: "1", href: "javascript:alert(1)" })).toThrow(/links to a route/);
  });
});

/** The stylesheet as written: vitest blanks CSS imports, and packages carry no Node types. */
function readStylesheet(): string {
  const node = (globalThis as unknown as { process: { getBuiltinModule(id: "node:fs"): { readFileSync(path: string, encoding: "utf8"): string } } }).process;
  const dir = (import.meta as ImportMeta & { dirname: string }).dirname;
  return node.getBuiltinModule("node:fs").readFileSync(`${dir}/charts.css`, "utf8");
}

test("the stylesheet maps every tone to a token and stops motion when it is reduced", () => {
  const css = readStylesheet();
  for (const tone of ["success", "warning", "danger", "info", "neutral"]) expect(css).toContain(`[data-tone="${tone}"] { --chart-color: var(--color-`);
  expect(css).toMatch(/prefers-reduced-motion: reduce\) \{\s*\.ui-chart-line, \.ui-chart-area, \.ui-chart-mark, \.ui-chart-slice, \.ui-chart-tile \{ animation: none; \}/);
  expect(css).toContain("var(--motion-slow)");
});
