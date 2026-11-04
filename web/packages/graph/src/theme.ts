// Reads the graph's colours from the design tokens on an element (CONTRACTS.md §4). A missing
// token falls back to another token, and finally to the element's own text colour.

import { invariant } from "./invariant.ts";
import type { GraphTheme } from "./renderer.ts";

/** Calls `onChange` when `data-theme` changes on the element or an ancestor, which re-themes it. */
export function watchTheme(element: Element, onChange: () => void): () => void {
  invariant(element.isConnected, "watchTheme needs an element in the document");
  const observer = new MutationObserver((records) => {
    if (records.some((record) => record.target.contains(element))) onChange();
  });
  observer.observe(element.ownerDocument.documentElement, { attributes: true, attributeFilter: ["data-theme"], subtree: true });
  return () => observer.disconnect();
}

export function readTheme(element: Element): GraphTheme {
  const style = getComputedStyle(element);
  const token = (...names: string[]): string => {
    for (const name of names) {
      const value = style.getPropertyValue(name).trim();
      if (value !== "") return value;
    }
    return style.color;
  };
  const statuses = new Map<string, string>();
  const muted = token("--color-text-muted");
  return {
    surface: token("--color-surface-1", "--color-bg"),
    border: token("--color-border"),
    text: token("--color-text"),
    muted,
    accent: token("--color-accent"),
    focus: token("--color-focus", "--color-accent"),
    edge: token("--color-border-strong", "--color-border"),
    font: `${style.getPropertyValue("--text-sm").trim() || "12px"} ${style.getPropertyValue("--font-ui").trim() || "sans-serif"}`,
    status: (status) => {
      if (status === undefined) return muted;
      let colour = statuses.get(status);
      if (colour === undefined) statuses.set(status, (colour = token(`--status-${status}`, "--color-text-muted")));
      return colour;
    },
  };
}
