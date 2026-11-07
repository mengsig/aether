import { useEffect, useRef, useState, type Key, type ReactNode } from "react";
import { classes, invariant } from "./classes.ts";

// Browsers clamp an element's height near 2^24 px in Firefox; stay under it.
const MAX_SCROLL_HEIGHT = 15_000_000;

export interface VirtualRange {
  readonly start: number;
  /** Exclusive. */
  readonly end: number;
}

/** The rows a viewport shows, widened by `overscan` on each side and clamped to the list. */
export function visibleRange(scrollTop: number, viewport: number, rowHeight: number, count: number, overscan: number): VirtualRange {
  invariant(rowHeight > 0 && Number.isFinite(rowHeight), `row height ${rowHeight} must be a positive number`);
  invariant(count >= 0 && Number.isInteger(count), `row count ${count} must be a whole number`);
  invariant(overscan >= 0 && viewport >= 0, "overscan and viewport are not negative");
  const first = Math.floor(Math.max(0, scrollTop) / rowHeight);
  const shown = Math.ceil(viewport / rowHeight) + 1;
  const start = Math.max(0, Math.min(count, first - overscan));
  const end = Math.min(count, first + shown + overscan);
  return { start, end: Math.max(start, end) };
}

export interface VirtualListProps {
  readonly label: string;
  readonly count: number;
  /** Every row has this height in px. */
  readonly rowHeight: number;
  readonly renderRow: (index: number) => ReactNode;
  readonly rowKey?: (index: number) => Key;
  /** A fixed viewport height in px; without it the list fills its parent and measures itself. */
  readonly height?: number;
  readonly overscan?: number;
  readonly className?: string;
}

const FALLBACK_VIEWPORT = 480;

/** A fixed-row-height virtual list: the DOM holds only the rows in view plus a small overscan. */
export function VirtualList({ label, count, rowHeight, renderRow, rowKey, height, overscan = 4, className }: VirtualListProps) {
  invariant(count * rowHeight <= MAX_SCROLL_HEIGHT, `${count} rows of ${rowHeight}px exceed the browser's scroll height`);
  const ref = useRef<HTMLDivElement>(null);
  // Kept whole rows apart, so scrolling within a row does not re-render.
  const [scrollTop, setScrollTop] = useState(0);
  const [measured, setMeasured] = useState<number | null>(null);

  useEffect(() => {
    const element = ref.current;
    invariant(element !== null, "the list viewport is mounted");
    if (typeof ResizeObserver !== "function") return undefined;
    const observer = new ResizeObserver(() => setMeasured(element.clientHeight));
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  const viewport = measured || height || FALLBACK_VIEWPORT;
  const { start, end } = visibleRange(scrollTop, viewport, rowHeight, count, overscan);
  const rows: ReactNode[] = [];
  for (let index = start; index < end; index += 1) {
    rows.push(
      <div key={rowKey?.(index) ?? index} role="listitem" className="ui-vlist-row" aria-posinset={index + 1} aria-setsize={count}
        style={{ insetBlockStart: index * rowHeight, blockSize: rowHeight }}>
        {renderRow(index)}
      </div>,
    );
  }
  return (
    <div ref={ref} className={classes("ui-vlist", className)} role="list" aria-label={label} tabIndex={0}
      style={height === undefined ? undefined : { blockSize: height }}
      onScroll={(event) => setScrollTop(Math.floor(event.currentTarget.scrollTop / rowHeight) * rowHeight)}>
      <div className="ui-vlist-spacer" style={{ blockSize: count * rowHeight }}>{rows}</div>
    </div>
  );
}
