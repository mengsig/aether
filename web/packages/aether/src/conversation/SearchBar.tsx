// Find in the transcript: a small field in the transcript's header with a match count and
// previous/next. Stepping opens the folded part that holds the match and scrolls its row into view.

import { Button } from "@aether-ui/ui";
import { useCallback, useDeferredValue, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type RefObject } from "react";
import { invariant } from "../fleet/invariant.ts";
import { clearMatches, paintMatches } from "./highlight.ts";
import type { ListHandle } from "./MeasuredList.tsx";
import type { ConversationRow } from "./rows.ts";
import { foldsToOpen, locate, NO_RESULT, searchRows, stepMatch, type SearchResult } from "./search.ts";
import type { ToggleSet } from "./toggles.ts";

export interface TranscriptSearch {
  readonly open: boolean;
  readonly query: string;
  readonly result: SearchResult;
  /** The current match (0-based), or -1 when there is none. */
  readonly current: number;
  /** The row of the current match, for the list to mark. */
  readonly currentRow: number;
  readonly show: () => void;
  readonly hide: () => void;
  readonly setQuery: (query: string) => void;
  readonly step: (step: 1 | -1) => void;
}

interface SearchDeps {
  readonly rows: readonly ConversationRow[];
  readonly list: RefObject<ListHandle | null>;
  readonly stage: RefObject<HTMLElement | null>;
  readonly toggles: ToggleSet;
}

/** Repaints the matches whenever the drawn rows change, and scrolls the current one into view once. */
function usePaint(stage: RefObject<HTMLElement | null>, result: SearchResult, current: number, rows: readonly ConversationRow[]): void {
  const scrolled = useRef("");
  const at = current < 0 ? null : locate(result, current);
  const rowKey = at === null ? null : rows[at.row]?.key ?? null;
  const nth = at?.nth ?? 0;
  useLayoutEffect(() => {
    const root = stage.current?.querySelector<HTMLElement>(".conv-list");
    // A cleared search forgets what it scrolled to, so the same term found again scrolls again.
    if (current < 0) scrolled.current = "";
    if (!root || result.query === "") return undefined;
    const paint = () => {
      const target = paintMatches(root, result.query, rowKey === null ? null : { rowKey, nth });
      // The row key names the layout too, so a mode change scrolls to the match's new row.
      const mark = `${result.query}#${current}@${rowKey ?? ""}`;
      if (target === null || scrolled.current === mark) return;
      scrolled.current = mark;
      target.scrollIntoView?.({ block: "center" });
    };
    paint();
    const observer = new MutationObserver(paint);
    observer.observe(root, { childList: true, subtree: true, characterData: true });
    return () => {
      observer.disconnect();
      clearMatches();
    };
  }, [stage, result.query, rowKey, nth, current]);
}

/** Opens the folds that hold the current match and brings its row into view, once per match. */
function useReveal({ rows, list, toggles }: SearchDeps, result: SearchResult, current: number): void {
  const revealed = useRef("");
  const row = current < 0 ? -1 : locate(result, current).row;
  useLayoutEffect(() => {
    const target = rows[row];
    if (target === undefined) {
      revealed.current = "";
      return;
    }
    // The row key names the layout: a mode change folds the match under a new key, so it opens again.
    const mark = `${result.query}#${current}@${target.key}`;
    if (revealed.current === mark) return;
    revealed.current = mark;
    for (const key of foldsToOpen(target, result.query)) toggles.set(key, true);
    list.current?.scrollToIndex(row);
  }, [rows, list, toggles, result.query, current, row]);
}

export function useTranscriptSearch(deps: SearchDeps): TranscriptSearch {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [picked, setPicked] = useState<{ readonly query: string; readonly match: number } | null>(null);
  const deferred = useDeferredValue(query);
  const result = useMemo(() => (open ? searchRows(deps.rows, deferred) : NO_RESULT), [open, deps.rows, deferred]);
  // A new query lands on its newest match, until the person steps to another.
  const current = picked !== null && picked.query === result.query && picked.match < result.total ? picked.match : result.total - 1;
  invariant(current >= -1 && current < Math.max(1, result.total), `match ${current} of ${result.total} is in range`);
  const step = useCallback((by: 1 | -1) => {
    if (result.total === 0) return;
    setPicked({ query: result.query, match: stepMatch(current < 0 ? (by === 1 ? -1 : 0) : current, by, result.total) });
  }, [result, current]);
  const show = useCallback(() => setOpen(true), []);
  const hide = useCallback(() => {
    setOpen(false);
    setQuery("");
    setPicked(null);
  }, []);
  useReveal(deps, result, current);
  usePaint(deps.stage, result, current, deps.rows);
  return { open, query, result, current, currentRow: current < 0 ? -1 : locate(result, current).row, show, hide, setQuery, step };
}

export function SearchBar({ search, onClose }: { readonly search: TranscriptSearch; readonly onClose: () => void }) {
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    input.current?.focus();
    input.current?.select();
  }, []);
  const { result, current, query } = search;
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter") {
      event.preventDefault();
      search.step(event.shiftKey ? -1 : 1);
    } else if (event.key === "Escape") {
      event.preventDefault();
      onClose();
    }
  };
  const count = query.trim() === "" ? "" : result.total === 0 ? "No matches" : `${current + 1} of ${result.total.toLocaleString("en")}`;
  return (
    <div className="conv-search" role="search">
      <input ref={input} type="search" className="conv-search-input" data-testid="conv-search" aria-label="Find in the transcript"
        placeholder="Find in transcript" value={query} onChange={(event) => search.setQuery(event.target.value)} onKeyDown={onKeyDown} />
      <span className="conv-search-count" aria-live="polite" data-testid="conv-search-count">{count}</span>
      <Button variant="quiet" size="sm" aria-label="Previous match" title="Previous match (Shift+Enter)" data-testid="conv-search-prev" disabled={result.total === 0} onClick={() => search.step(-1)}>↑</Button>
      <Button variant="quiet" size="sm" aria-label="Next match" title="Next match (Enter)" data-testid="conv-search-next" disabled={result.total === 0} onClick={() => search.step(1)}>↓</Button>
      <Button variant="quiet" size="sm" aria-label="Close find" title="Close (Esc)" onClick={onClose}>✕</Button>
    </div>
  );
}
