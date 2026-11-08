// Small inline-SVG charts: a sparkline, columns, a donut and the stat tile that holds them. Each is an
// image with a title and description, and its values are also given as text for assistive tech.

import { useId, type CSSProperties, type ReactNode } from "react";
import { invariant } from "./classes.ts";
import "./charts.css";

export type ChartTone = "accent" | "success" | "warning" | "danger" | "info" | "neutral";
export interface ChartPoint { readonly label: string; readonly value: number }

const SPARK_WIDTH = 100;
const SPARK_PAD = 2;
const BAR_SLOT = 10;
const BAR_WIDTH = 7;
const DONUT_GAP = 1;

const number = (value: number): string => value.toLocaleString("en", { maximumFractionDigits: 2 });

function checkPoints(points: readonly ChartPoint[], what: string): void {
  invariant(points.every((point) => Number.isFinite(point.value)), `every ${what} value is a finite number`);
  invariant(new Set(points.map((point) => point.label)).size === points.length, `${what} labels are unique`);
}

function ValueTable(props: { readonly label: string; readonly points: readonly ChartPoint[] }) {
  return (
    <table className="ui-visually-hidden">
      <caption>{props.label}</caption>
      <thead><tr><th scope="col">Label</th><th scope="col">Value</th></tr></thead>
      <tbody>
        {props.points.map((point) => <tr key={point.label}><th scope="row">{point.label}</th><td>{number(point.value)}</td></tr>)}
      </tbody>
    </table>
  );
}

/** The line through `values` in a SPARK_WIDTH × height box, with zero or the lowest value at the bottom. */
export function sparkPath(values: readonly number[], height: number): string {
  invariant(height > SPARK_PAD * 2, `a sparkline is taller than its padding, not ${height}`);
  if (values.length === 0) return `M0 ${height - SPARK_PAD} L${SPARK_WIDTH} ${height - SPARK_PAD}`;
  const low = Math.min(0, ...values);
  const span = Math.max(...values) - low || 1;
  const y = (value: number) => (SPARK_PAD + (1 - (value - low) / span) * (height - SPARK_PAD * 2)).toFixed(2);
  // One value is a level line across the box.
  const series = values.length === 1 ? [values[0] ?? 0, values[0] ?? 0] : values;
  const step = SPARK_WIDTH / (series.length - 1);
  const path = `M${series.map((value, i) => `${i * step} ${y(value)}`).join(" L")}`;
  invariant(!path.includes("NaN"), "the sparkline path has real coordinates");
  return path;
}

function describeSeries(points: readonly ChartPoint[]): string {
  if (points.length === 0) return "No data yet.";
  const values = points.map((point) => point.value);
  const last = points[points.length - 1];
  const first = points[0];
  invariant(first !== undefined && last !== undefined, "a series with points has a first and a last");
  return `${points.length} values from ${first.label} to ${last.label}; latest ${number(last.value)}, highest ${number(Math.max(...values))}.`;
}

export function Sparkline(props: { readonly label: string; readonly points: readonly ChartPoint[]; readonly tone?: ChartTone; readonly height?: number }): ReactNode {
  const { label, points } = props;
  const height = props.height ?? 32;
  invariant(label.trim() !== "", "a sparkline is named");
  checkPoints(points, "sparkline");
  const id = useId();
  const line = sparkPath(points.map((point) => point.value), height);
  const slot = points.length > 0 ? SPARK_WIDTH / points.length : SPARK_WIDTH;
  return (
    <figure className="ui-chart ui-chart-sparkline ui-chart-tone" data-tone={props.tone ?? "accent"}>
      <svg role="img" aria-labelledby={`${id}t ${id}d`} viewBox={`0 0 ${SPARK_WIDTH} ${height}`} preserveAspectRatio="none" style={{ blockSize: `${height}px` }}>
        <title id={`${id}t`}>{label}</title>
        <desc id={`${id}d`}>{describeSeries(points)}</desc>
        <path className="ui-chart-area" d={`${line} L${SPARK_WIDTH} ${height} L0 ${height} Z`} />
        <path className="ui-chart-line" d={line} pathLength={1} vectorEffect="non-scaling-stroke" />
        {points.map((point, i) => (
          <rect key={point.label} className="ui-chart-hit" x={i * slot} y={0} width={slot} height={height}>
            <title>{`${point.label}: ${number(point.value)}`}</title>
          </rect>
        ))}
      </svg>
      <ValueTable label={label} points={points} />
    </figure>
  );
}

export function BarChart(props: { readonly label: string; readonly bars: readonly (ChartPoint & { readonly tone?: ChartTone })[]; readonly max?: number }): ReactNode {
  const { label, bars } = props;
  invariant(label.trim() !== "", "a bar chart is named");
  checkPoints(bars, "bar");
  invariant(bars.every((bar) => bar.value >= 0), "bars measure nonnegative amounts");
  const max = props.max ?? Math.max(0, ...bars.map((bar) => bar.value));
  invariant(bars.every((bar) => bar.value <= max), `no bar is above the scale's maximum ${max}`);
  const id = useId();
  const slots = Math.max(1, bars.length);
  const desc = bars.length === 0 ? "No data yet." : bars.map((bar) => `${bar.label} ${number(bar.value)}`).join(", ");
  return (
    <figure className="ui-chart ui-chart-bars" style={{ "--bars": slots } as CSSProperties}>
      <svg role="img" aria-labelledby={`${id}t ${id}d`} viewBox={`0 0 ${slots * BAR_SLOT} 100`} preserveAspectRatio="none">
        <title id={`${id}t`}>{label}</title>
        <desc id={`${id}d`}>{desc}</desc>
        {bars.map((bar, i) => {
          const tall = max > 0 ? (bar.value / max) * 100 : 0;
          return (
            <g key={bar.label}>
              <rect className="ui-chart-track" x={i * BAR_SLOT + (BAR_SLOT - BAR_WIDTH) / 2} y={0} width={BAR_WIDTH} height={100} />
              <rect className="ui-chart-mark ui-chart-tone" data-tone={bar.tone ?? "accent"} x={i * BAR_SLOT + (BAR_SLOT - BAR_WIDTH) / 2} y={100 - tall} width={BAR_WIDTH} height={tall}>
                <title>{`${bar.label}: ${number(bar.value)}`}</title>
              </rect>
            </g>
          );
        })}
      </svg>
      <div className="ui-chart-axis" aria-hidden="true">
        {bars.map((bar) => <span key={bar.label}><span className="ui-chart-axis-value">{number(bar.value)}</span>{bar.label}</span>)}
      </div>
      <ValueTable label={label} points={bars} />
    </figure>
  );
}

export function StatTile(props: { readonly label: string; readonly value: string; readonly hint?: string; readonly tone?: ChartTone; readonly href?: string; readonly children?: ReactNode }): ReactNode {
  const { label, value, hint, href, tone } = props;
  invariant(label.trim() !== "" && value.trim() !== "", "a stat tile has a label and a value");
  invariant(href === undefined || href.startsWith("#/") || /^https?:\/\//.test(href), `a stat tile links to a route or a web page, not ${href}`);
  const id = useId();
  return (
    <article className="ui-chart-tile ui-chart-tone" data-tone={tone} aria-labelledby={id}>
      <h3 className="ui-chart-tile-label" id={id}>
        {tone === undefined ? null : <span className="ui-chart-tile-dot" aria-hidden="true" />}
        {href === undefined ? label : <a className="ui-chart-tile-link" href={href}>{label}<span className="ui-chart-tile-arrow" aria-hidden="true">›</span></a>}
      </h3>
      <p className="ui-chart-tile-value">{value}</p>
      {hint === undefined ? null : <p className="ui-chart-tile-hint">{hint}</p>}
      {props.children === undefined ? null : <div className="ui-chart-tile-body">{props.children}</div>}
    </article>
  );
}

export interface DonutArc { readonly label: string; readonly tone: ChartTone; readonly start: number; readonly length: number }

/** Each nonzero slice as an arc on a circle of length 100, a small gap between neighbours. */
export function donutArcs(slices: readonly (ChartPoint & { readonly tone: ChartTone })[]): DonutArc[] {
  const total = slices.reduce((sum, slice) => sum + slice.value, 0);
  const shown = slices.filter((slice) => slice.value > 0);
  if (total <= 0) return [];
  const gap = shown.length > 1 ? DONUT_GAP : 0;
  let start = 0;
  const arcs = shown.map((slice) => {
    const share = (slice.value / total) * 100;
    const arc = { label: slice.label, tone: slice.tone, start, length: Math.max(share - gap, gap > 0 ? 0.5 : share) };
    start += share;
    return arc;
  });
  invariant(Math.abs(start - 100) < 1e-6, "the slices go once around");
  return arcs;
}

export function Donut(props: { readonly label: string; readonly slices: readonly (ChartPoint & { readonly tone: ChartTone })[]; readonly center?: string }): ReactNode {
  const { label, slices, center } = props;
  invariant(label.trim() !== "", "a donut is named");
  checkPoints(slices, "slice");
  invariant(slices.every((slice) => slice.value >= 0), "slices measure nonnegative amounts");
  const id = useId();
  const arcs = donutArcs(slices);
  const total = slices.reduce((sum, slice) => sum + slice.value, 0);
  const share = (value: number) => (total > 0 ? Math.round((value / total) * 100) : 0);
  const desc = [center, total > 0 ? slices.map((s) => `${s.label} ${number(s.value)} (${share(s.value)}%)`).join(", ") : "No data yet."].filter(Boolean).join(". ");
  return (
    <figure className="ui-chart ui-chart-donut">
      <div className="ui-chart-donut-ring">
        <svg role="img" aria-labelledby={`${id}t ${id}d`} viewBox="0 0 100 100">
          <title id={`${id}t`}>{label}</title>
          <desc id={`${id}d`}>{desc}</desc>
          <circle className="ui-chart-track" cx={50} cy={50} r={40} pathLength={100} />
          {arcs.map((arc) => (
            <circle key={arc.label} className="ui-chart-slice ui-chart-tone" data-tone={arc.tone} cx={50} cy={50} r={40} pathLength={100}
              strokeDasharray={`${arc.length} ${100 - arc.length}`} strokeDashoffset={-arc.start} transform="rotate(-90 50 50)">
              <title>{arc.label}</title>
            </circle>
          ))}
        </svg>
        {center === undefined ? null : <span className="ui-chart-donut-center" aria-hidden="true">{center}</span>}
      </div>
      <ul className="ui-chart-legend">
        {slices.map((slice) => (
          <li key={slice.label}>
            <span className="ui-chart-swatch ui-chart-tone" data-tone={slice.tone} aria-hidden="true" />
            <span className="ui-chart-legend-label">{slice.label}</span>
            <span className="ui-chart-legend-value">{share(slice.value)}%</span>
          </li>
        ))}
      </ul>
    </figure>
  );
}
