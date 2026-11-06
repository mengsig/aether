import type { ReactNode } from "react";
import { invariant } from "../issue/ref.ts";

type Tone = "accent" | "success" | "warning" | "danger" | "info" | "neutral";
interface Point { readonly label: string; readonly value: number }
const color = (tone: Tone = "accent") => `var(--color-${tone === "neutral" ? "text-muted" : tone})`;

export function Sparkline({ label, points, tone = "accent", height = 80 }: { readonly label: string; readonly points: readonly Point[]; readonly tone?: Tone; readonly height?: number }) {
  invariant(points.every((point) => point.value >= 0 && Number.isFinite(point.value)), "sparkline values are finite and nonnegative");
  invariant(height > 0, "a sparkline has positive height");
  const max = Math.max(1, ...points.map((point) => point.value));
  const line = points.map((point, i) => `${10 + i * 280 / Math.max(1, points.length - 1)},${height - 10 - point.value / max * (height - 20)}`).join(" ");
  return <figure className="gh-chart"><figcaption>{label}</figcaption><svg role="img" aria-label={label} viewBox={`0 0 300 ${height}`}>
    <title>{label}</title><desc>{points.map((point) => `${point.label}: ${point.value}`).join("; ")}</desc>
    <polyline points={line} fill="none" stroke={color(tone)} strokeWidth="2" />
  </svg></figure>;
}

export function BarChart({ label, bars, max }: { readonly label: string; readonly bars: readonly (Point & { readonly tone?: Tone })[]; readonly max?: number }) {
  invariant(bars.every((bar) => Number.isFinite(bar.value) && bar.value >= 0), "bar values are finite and nonnegative");
  const ceiling = max ?? Math.max(1, ...bars.map((bar) => bar.value));
  invariant(ceiling > 0, "bars have a positive scale");
  return <figure className="gh-chart"><figcaption>{label}</figcaption><svg role="img" aria-label={label} viewBox={`0 0 360 ${Math.max(1, bars.length) * 32}`}>
    <title>{label}</title><desc>{bars.map((bar) => `${bar.label}: ${bar.value}`).join("; ")}</desc>
    {bars.map((bar, i) => <g key={bar.label}><text x="0" y={i * 32 + 13} fill="currentColor" fontSize="10">{bar.label}</text>
      <rect x="0" y={i * 32 + 18} width={Math.min(1, bar.value / ceiling) * 320} height="7" rx="3" fill={color(bar.tone)} />
      <text x="335" y={i * 32 + 25} fill="currentColor" fontSize="12">{bar.value}</text></g>)}
  </svg></figure>;
}

export function StatTile({ label, value, hint, tone, href, children }: { readonly label: string; readonly value: ReactNode; readonly hint?: string; readonly tone?: Tone; readonly href?: string; readonly children?: ReactNode }) {
  const content = <><span>{label}</span><strong style={{ color: color(tone) }}>{value}</strong>{hint ? <small>{hint}</small> : null}{children}</>;
  return href ? <a className="gh-stat ui-link" href={href}>{content}</a> : <div className="gh-stat">{content}</div>;
}

export function Donut({ label, slices, center }: { readonly label: string; readonly slices: readonly (Point & { readonly tone: Tone })[]; readonly center?: string }) {
  invariant(slices.every((slice) => slice.value >= 0 && Number.isFinite(slice.value)), "donut slices are finite and nonnegative");
  const total = slices.reduce((sum, slice) => sum + slice.value, 0);
  invariant(Number.isFinite(total), "donut total is finite");
  const segments = slices.reduce<{ label: string; tone: Tone; share: number; start: number }[]>((result, slice) => {
    const previous = result.at(-1);
    result.push({ label: slice.label, tone: slice.tone, share: total === 0 ? 0 : slice.value / total * 100, start: previous ? previous.start + previous.share : 0 });
    return result;
  }, []);
  return <figure className="gh-chart"><figcaption>{label}</figcaption><svg role="img" aria-label={label} viewBox="0 0 180 160">
    <title>{label}</title><desc>{slices.map((slice) => `${slice.label}: ${slice.value}`).join("; ")}</desc>
    {segments.map((segment) => <circle key={segment.label} cx="90" cy="80" r="55" pathLength="100" fill="none" stroke={color(segment.tone)} strokeWidth="18"
      strokeDasharray={`${segment.share} ${100 - segment.share}`} strokeDashoffset={-segment.start} transform="rotate(-90 90 80)" />)}<text x="90" y="85" textAnchor="middle" fill="currentColor">{center ?? total}</text>
  </svg><div className="gh-chart-legend">{slices.map((slice) => <span key={slice.label}>{slice.label} {slice.value}</span>)}</div></figure>;
}
