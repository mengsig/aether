// Reads the graph's colours from the design tokens on an element (CONTRACTS.md §4). A missing
// token falls back to another token, and finally to the element's own text colour.

import type { GraphTheme } from "./renderer.ts";

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
