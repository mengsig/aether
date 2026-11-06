// Paints search matches in the drawn rows with the CSS Custom Highlight API, which marks ranges
// without touching React's DOM. Where the API is missing, only the current row is marked (by the list).

import { invariant } from "../fleet/invariant.ts";
import { foldCase } from "./search.ts";

const ALL = "conv-search";
const CURRENT = "conv-search-current";

interface HighlightRegistry { set(name: string, highlight: unknown): void; delete(name: string): boolean }
type HighlightConstructor = new (...ranges: Range[]) => unknown;

function api(): { registry: HighlightRegistry; Highlight: HighlightConstructor } | null {
  const registry = (globalThis.CSS as { highlights?: HighlightRegistry } | undefined)?.highlights;
  const Highlight = (globalThis as { Highlight?: HighlightConstructor }).Highlight;
  return registry === undefined || Highlight === undefined ? null : { registry, Highlight };
}

/** The original offsets of a match at `start..end` of the folded text, widened to whole original characters. */
function originalSpan(origin: readonly number[], start: number, end: number): [number, number] {
  invariant(start < end && end < origin.length, "a match lies inside the folded text");
  const from = origin[start] as number;
  const last = origin[end - 1] as number;
  let after = end;
  while ((origin[after] as number) === last) after += 1;
  const to = origin[after] as number;
  invariant(from < to, "a match spans at least one original character");
  return [from, to];
}

/** Every visible occurrence of `needle` (case-folded) in the text under `root`, in document order. */
function rangesIn(root: Element, needle: string): Range[] {
  invariant(needle !== "" && needle === foldCase(needle).folded, "the needle is case-folded and not empty");
  const ranges: Range[] = [];
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node !== null; node = walker.nextNode()) {
    const { folded, origin } = foldCase(node.nodeValue ?? "");
    if (!folded.includes(needle) || node.parentElement?.closest("[hidden]")) continue;
    for (let at = folded.indexOf(needle); at >= 0; at = folded.indexOf(needle, at + needle.length)) {
      const [from, to] = originalSpan(origin, at, at + needle.length);
      const range = document.createRange();
      range.setStart(node, from);
      range.setEnd(node, to);
      ranges.push(range);
    }
  }
  return ranges;
}

export interface CurrentMatch { readonly rowKey: string; readonly nth: number }

/** Paints the matches under `list`; returns the element holding the current match, to scroll to. */
export function paintMatches(list: HTMLElement, needle: string, current: CurrentMatch | null): Element | null {
  invariant(list.isConnected, "matches are painted in a drawn list");
  invariant(current === null || (Number.isInteger(current.nth) && current.nth >= 0), "the current match is a place in its row");
  const row = current === null ? null : [...list.querySelectorAll<HTMLElement>("[data-key]")].find((node) => node.dataset.key === current.rowKey) ?? null;
  const inRow = row === null || current === null ? [] : rangesIn(row, needle);
  const target = current === null ? undefined : inRow[Math.min(current.nth, inRow.length - 1)];
  const highlight = api();
  if (highlight !== null) {
    highlight.registry.set(ALL, new highlight.Highlight(...rangesIn(list, needle)));
    if (target) highlight.registry.set(CURRENT, new highlight.Highlight(target));
    else highlight.registry.delete(CURRENT);
  }
  return target?.startContainer.parentElement ?? row;
}

export function clearMatches(): void {
  const highlight = api();
  if (highlight === null) return;
  highlight.registry.delete(ALL);
  highlight.registry.delete(CURRENT);
}
