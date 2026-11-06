// The transcript's virtual list: it follows the newest message while at the end, offers a jump
// back with the count of messages that arrived meanwhile, and remembers where the reader was per agent.

import type { ConnId } from "@aether-ui/protocol";
import { Skeleton } from "@aether-ui/ui";
import { useCallback, useState, type RefObject } from "react";
import { invariant } from "../fleet/invariant.ts";
import { MeasuredList, type ListHandle, type ScrollAnchor } from "./MeasuredList.tsx";
import { estimateRow, MessageView } from "./Message.tsx";
import type { ConversationRow } from "./rows.ts";

/** Where each transcript was left this session, by `<conn>/<agent>`: a row key and offset, or its end. */
const scrollMemory = new Map<string, { readonly key: string; readonly offset: number } | "end">();
const estimates = new WeakMap<ConversationRow, number>();

function cachedEstimate(row: ConversationRow): number {
  let height = estimates.get(row);
  if (height === undefined) {
    height = estimateRow(row);
    estimates.set(row, height);
  }
  return height;
}

/** The remembered place as a row index, or null to open at the end. */
function restoredAnchor(memoryKey: string, rows: readonly ConversationRow[]): Omit<ScrollAnchor, "atEnd"> | null {
  const saved = scrollMemory.get(memoryKey);
  if (saved === undefined || saved === "end") return null;
  const index = rows.findIndex((row) => row.key === saved.key);
  return index < 0 ? null : { index, offset: saved.offset };
}

/** Forgets every remembered place; for tests. */
export function forgetScrollPositions(): void {
  scrollMemory.clear();
}

function JumpToLatest({ fresh, onJump }: { readonly fresh: number; readonly onJump: () => void }) {
  invariant(fresh >= 0, "a count of new messages is not negative");
  return (
    <button type="button" className="conv-jump" data-testid="conv-jump-latest" onClick={onJump}>
      <span className="conv-jump-arrow" aria-hidden="true">↓</span>
      Jump to latest{fresh > 0 ? <span className="conv-jump-count"> · {fresh.toLocaleString("en")} new</span> : null}
    </button>
  );
}

interface TranscriptListProps {
  readonly conn: ConnId;
  readonly agent: string;
  readonly rows: readonly ConversationRow[];
  readonly committed: number;
  readonly currentRow: number;
  readonly handle: RefObject<ListHandle | null>;
}

export function TranscriptList({ conn, agent, rows, committed, currentRow, handle }: TranscriptListProps) {
  const memoryKey = `${conn}/${agent}`;
  const [initial] = useState(() => restoredAnchor(memoryKey, rows));
  const [atEnd, setAtEnd] = useState(initial === null);
  // The committed count when the reader left the end; what arrived since is "new".
  const [baseline, setBaseline] = useState<number | null>(initial === null ? null : committed);
  invariant(committed <= rows.length, "committed rows are a part of all rows");
  const onEndChange = useCallback((end: boolean) => {
    setAtEnd(end);
    setBaseline(end ? null : committed);
  }, [committed]);
  const onAnchorChange = useCallback((anchor: ScrollAnchor) => {
    const key = rows[anchor.index]?.key;
    scrollMemory.set(memoryKey, anchor.atEnd || key === undefined ? "end" : { key, offset: anchor.offset });
  }, [rows, memoryKey]);
  const rowKey = useCallback((index: number) => rows[index]?.key ?? String(index), [rows]);
  const estimate = useCallback((index: number) => {
    const row = rows[index];
    return row === undefined ? 0 : cachedEstimate(row);
  }, [rows]);
  const renderRow = useCallback((index: number) => {
    const row = rows[index];
    invariant(row !== undefined, `the list asked for row ${index} of ${rows.length}`);
    return <MessageView row={row} current={index === currentRow} />;
  }, [rows, currentRow]);
  const fresh = baseline === null ? 0 : Math.max(0, committed - baseline);
  return (
    <>
      <MeasuredList ref={handle} label="Messages" count={rows.length} rowKey={rowKey} estimate={estimate} renderRow={renderRow} follow
        initialScroll={initial} onEndChange={onEndChange} onAnchorChange={onAnchorChange} />
      {atEnd ? null : <JumpToLatest fresh={fresh} onJump={() => handle.current?.scrollToEnd()} />}
      <span className="ui-visually-hidden" role="status" aria-live="polite">{fresh > 0 ? `${fresh} new message${fresh === 1 ? "" : "s"} below` : ""}</span>
    </>
  );
}

/** Placeholders shaped like a few turns while the first page of the transcript loads. */
export function TranscriptSkeleton() {
  return (
    <div className="conv-skeleton" aria-busy="true" aria-label="Reading the transcript">
      {(["user", "assistant", "user", "assistant"] as const).map((speaker, index) => (
        <div key={index} className="conv-skeleton-msg" data-speaker={speaker}>
          <Skeleton className="conv-skeleton-mark" width="1.75rem" height="1.75rem" />
          <div className="conv-skeleton-lines">
            <Skeleton width="8rem" height="0.75rem" />
            <Skeleton height="0.9rem" />
            <Skeleton width={index % 2 === 0 ? "45%" : "80%"} height="0.9rem" />
          </div>
        </div>
      ))}
    </div>
  );
}
