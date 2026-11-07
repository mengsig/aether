// A virtual list whose rows have their own heights: rows start at an estimate, are measured once
// drawn, and only those in view (plus overscan) are in the DOM. Messages vary too much for fixed rows.

import { useCallback, useEffect, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState, type ReactNode, type Ref, type RefObject } from "react";
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

/** Where the view sits: the first row in view and how far into it, or at the end. */
export interface ScrollAnchor {
  readonly index: number;
  readonly offset: number;
  readonly atEnd: boolean;
}

export interface ListHandle {
  /** Brings row `index` into view, a little below the top, and stops following the end. */
  scrollToIndex(index: number): void;
  /** Goes to the end and follows it again. */
  scrollToEnd(): void;
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
  /** Where to open instead of the end (a following list) or the top. */
  readonly initialScroll?: Omit<ScrollAnchor, "atEnd"> | null;
  /** Told after each scroll where the view sits. */
  readonly onAnchorChange?: (anchor: ScrollAnchor) => void;
  /** Told when the view arrives at the end or leaves it. */
  readonly onEndChange?: (atEnd: boolean) => void;
  readonly ref?: Ref<ListHandle>;
  /** Names what the rows are drawn as; when it changes, the row in view stays in view. */
  readonly layout?: string;
  /** Where row i sits in an order every layout shares, non-decreasing; defaults to i. */
  readonly position?: (index: number) => number;
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

/** Puts the view at the restored anchor once, before the first paint. */
function useInitialScroll(ref: RefObject<HTMLDivElement | null>, offsets: Float64Array, initial: MeasuredListProps["initialScroll"], setTop: (top: number) => void): void {
  const applied = useRef(false);
  useLayoutEffect(() => {
    const element = ref.current;
    if (applied.current || !initial || element === null) return;
    applied.current = true;
    invariant(initial.index >= 0 && initial.offset >= 0, `the restored anchor ${initial.index}+${initial.offset} is in the list`);
    const top = (offsets[Math.min(initial.index, offsets.length - 1)] ?? 0) + initial.offset;
    element.scrollTop = top;
    setTop(top);
  }, [ref, offsets, initial, setTop]);
}

interface Geometry {
  readonly offsets: Float64Array;
  readonly viewport: number;
}

/** While following and at the end, keeps the view on the last row as rows grow or arrive. */
function useFollowEnd(ref: RefObject<HTMLDivElement | null>, follow: boolean, atEnd: RefObject<boolean>, { offsets, viewport }: Geometry,
  top: number, setTop: (top: number) => void): void {
  const total = offsets[offsets.length - 1] ?? 0;
  useLayoutEffect(() => {
    const element = ref.current;
    invariant(element !== null, "the list viewport is mounted");
    if (!follow || !atEnd.current) return;
    const end = Math.max(0, total - (element.clientHeight || viewport));
    invariant(end >= 0 && Number.isFinite(end), "the end of the list is a real position");
    element.scrollTop = end;
    if (top !== end) setTop(end);
  }, [ref, follow, atEnd, total, viewport, top, setTop]);
}

/** The parent's handle on the list: scroll to a row, or back to the end. */
function useListHandle(handle: Ref<ListHandle> | undefined, ref: RefObject<HTMLDivElement | null>, { offsets, viewport }: Geometry,
  setTop: (top: number) => void, setAtEnd: (atEnd: boolean) => void): void {
  useImperativeHandle(handle, () => {
    const count = offsets.length - 1;
    const total = offsets[count] ?? 0;
    return {
      scrollToIndex(index: number) {
        const element = ref.current;
        invariant(element !== null && index >= 0 && index < count, `row ${index} of ${count} can be scrolled to`);
        setAtEnd(false);
        const target = Math.max(0, (offsets[index] ?? 0) - viewport / 4);
        element.scrollTop = target;
        setTop(target);
      },
      scrollToEnd() {
        const element = ref.current;
        invariant(element !== null, "the list viewport is mounted");
        const end = Math.max(0, total - (element.clientHeight || viewport));
        element.scrollTop = end;
        setTop(end);
        setAtEnd(true);
      },
    };
  }, [ref, offsets, viewport, setTop, setAtEnd]);
}

/** The first row at or after `wanted` in the shared order, by binary search; `count` when none is. */
export function firstAtOrAfter(count: number, position: (index: number) => number, wanted: number): number {
  let low = 0;
  let high = count;
  while (low < high) {
    const mid = (low + high) >>> 1;
    if (position(mid) < wanted) low = mid + 1;
    else high = mid;
  }
  invariant(low === count || position(low) >= wanted, "the search lands on a row at or after the wanted one");
  return low;
}

interface Anchor { readonly layout: string; readonly position: number; readonly within: number }

/** Holds the row at the top of the view across a layout change, unless the list follows its end. */
function useLayoutAnchor(ref: RefObject<HTMLDivElement | null>, offsets: Float64Array, top: number, layout: string, position: (index: number) => number, following: () => boolean): void {
  const anchor = useRef<Anchor | null>(null);
  const count = offsets.length - 1;
  useLayoutEffect(() => {
    const element = ref.current;
    const held = anchor.current;
    if (element === null || held === null || held.layout === layout || following()) return;
    const index = firstAtOrAfter(count, position, held.position);
    element.scrollTop = Math.max(0, (offsets[index] ?? 0) + (position(index) === held.position ? held.within : 0));
  }, [ref, layout, offsets, count, position, following]);
  // Runs after the restore above, so it reads the scroll that restore set, not the stale `top`.
  useLayoutEffect(() => {
    if (count === 0) return;
    const y = ref.current?.scrollTop ?? top;
    const index = Math.min(count - 1, rowAt(offsets, y));
    anchor.current = { layout, position: position(index), within: y - (offsets[index] ?? 0) };
  }, [ref, offsets, top, layout, count, position]);
}

const identity = (index: number): number => index;

export function MeasuredList({ label, count, rowKey, estimate, renderRow, follow = false, className, initialScroll = null, onAnchorChange, onEndChange, ref: handle, layout = "", position = identity }: MeasuredListProps) {
  const ref = useRef<HTMLDivElement>(null);
  const [top, setTop] = useState(0);
  const viewport = useViewportHeight(ref);
  const atEnd = useRef(follow && initialScroll === null);
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
  useInitialScroll(ref, offsets, initialScroll, setTop);
  useFollowEnd(ref, follow, atEnd, { offsets, viewport }, top, setTop);
  const setAtEnd = useCallback((next: boolean) => {
    if (atEnd.current === next) return;
    atEnd.current = next;
    onEndChange?.(next);
  }, [onEndChange]);
  const onScroll = useCallback(() => {
    const element = ref.current;
    if (!element) return;
    setTop(element.scrollTop);
    setAtEnd(element.scrollTop + element.clientHeight >= element.scrollHeight - END_SLACK);
    const index = rowAt(offsets, element.scrollTop);
    onAnchorChange?.({ index, offset: Math.max(0, element.scrollTop - (offsets[index] ?? 0)), atEnd: atEnd.current });
  }, [offsets, onAnchorChange, setAtEnd]);
  useListHandle(handle, ref, { offsets, viewport }, setTop, setAtEnd);
  useLayoutEffect(() => {
    firstShown.current = rowAt(offsets, top);
  }, [offsets, top]);
  const following = useCallback(() => follow && atEnd.current, [follow]);
  useLayoutAnchor(ref, offsets, top, layout, position, following);

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
