// A virtual list whose rows have their own heights: rows start at an estimate, are measured once
// drawn, and only those in view (plus overscan) are in the DOM. Messages vary too much for fixed rows.

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode, type RefObject } from "react";
import { invariant } from "../fleet/invariant.ts";

const FALLBACK_VIEWPORT = 600;
const OVERSCAN_PX = 800;
/** How close to the end still counts as at the end, in px. */
const END_SLACK = 24;

/** Row tops: `offsets[i]` is where row i starts and `offsets[count]` the total height. */
export function rowOffsets(count: number, heightOf: (index: number) => number): Float64Array {
  invariant(Number.isInteger(count) && count >= 0, `row count ${count} is a whole number`);
  const offsets = new Float64Array(count + 1);
  for (let index = 0; index < count; index += 1) {
    const height = heightOf(index);
    invariant(height >= 0 && Number.isFinite(height), `row ${index} has a real height`);
    offsets[index + 1] = (offsets[index] ?? 0) + height;
  }
  return offsets;
}

/** The first row whose bottom is below `y`, by binary search over the offsets. */
export function rowAt(offsets: Float64Array, y: number): number {
  const count = offsets.length - 1;
  invariant(count >= 0, "offsets hold at least the total");
  let low = 0;
  let high = count;
  while (low < high) {
    const mid = (low + high) >>> 1;
    if ((offsets[mid + 1] ?? 0) <= y) low = mid + 1;
    else high = mid;
  }
  return low;
}

/** The rows that meet the band [top - overscan, top + viewport + overscan]. */
export function rowRange(offsets: Float64Array, top: number, viewport: number, overscan: number): { start: number; end: number } {
  const count = offsets.length - 1;
  const start = rowAt(offsets, Math.max(0, top - overscan));
  const end = Math.min(count, rowAt(offsets, top + viewport + overscan) + 1);
  invariant(start <= end && end <= count, `range ${start}..${end} lies within ${count} rows`);
  return { start, end };
}

export interface MeasuredListProps {
  readonly label: string;
  readonly count: number;
  readonly rowKey: (index: number) => string;
  readonly estimate: (index: number) => number;
  readonly renderRow: (index: number) => ReactNode;
  /** Keeps the last row in view while the list sits at its end, as a chat log does. */
  readonly follow?: boolean;
  readonly className?: string;
}

interface Measures {
  readonly heights: Map<string, number>;
  readonly version: number;
  /** A ref callback for each drawn row; its cleanup stops measuring the row. */
  readonly observe: (element: HTMLElement | null) => (() => void) | undefined;
}

/** Row heights by key, measured by one ResizeObserver; a changed height bumps `version`. */
function useMeasures(onResize: (index: number, change: number) => void): Measures {
  // Stable containers that change in place; `version` tells React when they did.
  const [heights] = useState(() => new Map<string, number>());
  const [drawn] = useState(() => new Set<HTMLElement>());
  const observer = useRef<ResizeObserver | null>(null);
  const [version, setVersion] = useState(0);
  useEffect(() => {
    if (typeof ResizeObserver !== "function") return undefined;
    const live = new ResizeObserver((records) => {
      let changed = false;
      for (const record of records) {
        const element = record.target as HTMLElement;
        const key = element.dataset.key;
        const height = element.offsetHeight;
        if (key === undefined || height <= 0 || heights.get(key) === height) continue;
        const before = heights.get(key) ?? Number(element.dataset.estimate);
        heights.set(key, height);
        onResize(Number(element.dataset.index), height - before);
        changed = true;
      }
      if (changed) setVersion((v) => v + 1);
    });
    for (const element of drawn) live.observe(element);
    observer.current = live;
    return () => {
      live.disconnect();
      observer.current = null;
    };
  }, [heights, drawn, onResize]);
  const observe = useCallback((element: HTMLElement | null) => {
    if (!element) return undefined;
    drawn.add(element);
    observer.current?.observe(element);
    return () => {
      drawn.delete(element);
      observer.current?.unobserve(element);
    };
  }, [drawn]);
  return { heights, version, observe };
}

/** The viewport's height, kept current as the window or the layout around the list resizes. */
function useViewportHeight(ref: RefObject<HTMLDivElement | null>): number {
  const [viewport, setViewport] = useState(FALLBACK_VIEWPORT);
  useLayoutEffect(() => {
    const element = ref.current;
    invariant(element !== null, "the list viewport is mounted");
    const measure = () => {
      if (element.clientHeight > 0) setViewport(element.clientHeight);
    };
    measure();
    window.addEventListener("resize", measure);
    const observer = typeof ResizeObserver === "function" ? new ResizeObserver(measure) : null;
    observer?.observe(element);
    return () => {
      window.removeEventListener("resize", measure);
      observer?.disconnect();
    };
  }, [ref]);
  invariant(viewport > 0, "the viewport has a height");
  return viewport;
}

export function MeasuredList({ label, count, rowKey, estimate, renderRow, follow = false, className }: MeasuredListProps) {
  const ref = useRef<HTMLDivElement>(null);
  const [top, setTop] = useState(0);
  const viewport = useViewportHeight(ref);
  const atEnd = useRef(follow);
  const firstShown = useRef(0);
  // A row above the view that grows would push the view down; scroll by as much to hold it still.
  const onResize = useCallback((index: number, change: number) => {
    const element = ref.current;
    if (!element || atEnd.current || !(index < firstShown.current) || !Number.isFinite(change)) return;
    element.scrollTop += change;
  }, []);
  const { heights, version, observe } = useMeasures(onResize);
  const offsets = useMemo(
    () => rowOffsets(count, (index) => heights.get(rowKey(index)) ?? estimate(index)),
    // `version` stands for the heights map, which changes in place.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [count, rowKey, estimate, heights, version],
  );
  const total = offsets[count] ?? 0;

  useLayoutEffect(() => {
    const element = ref.current;
    invariant(element !== null, "the list viewport is mounted");
    if (!follow || !atEnd.current) return;
    const end = Math.max(0, total - (element.clientHeight || viewport));
    element.scrollTop = end;
    if (top !== end) setTop(end);
  }, [follow, total, viewport, top]);

  const onScroll = useCallback(() => {
    const element = ref.current;
    if (!element) return;
    setTop(element.scrollTop);
    atEnd.current = element.scrollTop + element.clientHeight >= element.scrollHeight - END_SLACK;
  }, []);

  useLayoutEffect(() => {
    firstShown.current = rowAt(offsets, top);
  }, [offsets, top]);

  const { start, end } = rowRange(offsets, top, viewport, OVERSCAN_PX);
  const rows: ReactNode[] = [];
  for (let index = start; index < end; index += 1) {
    const key = rowKey(index);
    rows.push(
      <div key={key} ref={observe} role="listitem" className="conv-row" data-key={key} data-index={index} data-estimate={estimate(index)}
        aria-posinset={index + 1} aria-setsize={count} style={{ insetBlockStart: offsets[index] }}>
        {renderRow(index)}
      </div>,
    );
  }
  return (
    <div ref={ref} className={className === undefined ? "conv-list" : `conv-list ${className}`} role="list" aria-label={label} tabIndex={0} onScroll={onScroll}>
      <div className="conv-spacer" style={{ blockSize: total }}>{rows}</div>
    </div>
  );
}
