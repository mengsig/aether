// 16 px line icons for the known nav entries, drawn on one grid: 1.5 px strokes in currentColor.

import type { ReactNode } from "react";
import { invariant } from "./classes.ts";

const DOT = <circle cx="8" cy="8" r="2" />;

const ICONS: Readonly<Record<string, ReactNode>> = {
  fleet: <><rect x="2.5" y="2.5" width="4.5" height="4.5" rx="1.2" /><rect x="9" y="2.5" width="4.5" height="4.5" rx="1.2" /><rect x="2.5" y="9" width="4.5" height="4.5" rx="1.2" /><rect x="9" y="9" width="4.5" height="4.5" rx="1.2" /></>,
  topology: <><circle cx="4" cy="4" r="1.8" /><circle cx="12" cy="4" r="1.8" /><circle cx="8" cy="12" r="1.8" /><path d="M5.8 4h4.4M4.9 5.6l2.2 4.8M11.1 5.6l-2.2 4.8" /></>,
  inbox: <><path d="M4 3h8l1.5 6v3.5a1 1 0 0 1-1 1h-9a1 1 0 0 1-1-1V9z" /><path d="M2.5 9h3l1 2h3l1-2h3" /></>,
  activity: <path d="M1.5 8.5H4l2-4.5 3.5 8 2-3.5h3" />,
  memory: <><ellipse cx="8" cy="4" rx="5" ry="1.8" /><path d="M3 4v8c0 1 2.2 1.8 5 1.8s5-.8 5-1.8V4M3 8c0 1 2.2 1.8 5 1.8s5-.8 5-1.8" /></>,
  queue: <><path d="M6 4h7.5M6 8h7.5M6 12h7.5" /><circle cx="3" cy="4" r="0.6" /><circle cx="3" cy="8" r="0.6" /><circle cx="3" cy="12" r="0.6" /></>,
  history: <><path d="M2.8 8a5.2 5.2 0 1 0 1.5-3.7" /><path d="M2.5 2.8v2.6h2.6M8 5.2V8l2 1.3" /></>,
  "task graph": <><rect x="2" y="6.5" width="3.5" height="3" rx="0.8" /><rect x="10.5" y="2.5" width="3.5" height="3" rx="0.8" /><rect x="10.5" y="10.5" width="3.5" height="3" rx="0.8" /><path d="M5.5 8H8V4h2.5M8 8v4h2.5" /></>,
  learn: <path d="M8 4.2C6.6 3.2 4.6 2.8 2.5 3v9.3c2.1-.2 4.1.2 5.5 1.2 1.4-1 3.4-1.4 5.5-1.2V3c-2.1-.2-4.1.2-5.5 1.2zM8 4.2v9.3" />,
  sessions: <path d="M3 3.5h10a1 1 0 0 1 1 1v6a1 1 0 0 1-1 1H7.5L4.5 14v-2.5H3a1 1 0 0 1-1-1v-6a1 1 0 0 1 1-1z" />,
  knowledge: <path d="M8 2.5l5.5 3-5.5 3-5.5-3zM2.5 8.2l5.5 3 5.5-3M2.5 10.8l5.5 3 5.5-3" />,
  "my understanding": <><circle cx="8" cy="8" r="5.5" /><circle cx="8" cy="8" r="2.8" /><circle cx="8" cy="8" r="0.6" /></>,
  settings: <><path d="M2.5 4.5h6M11.5 4.5h2M2.5 11.5h2M7.5 11.5h6" /><circle cx="10" cy="4.5" r="1.5" /><circle cx="6" cy="11.5" r="1.5" /></>,
};

/** The nav entry's icon by its label; an entry the kit does not know gets a small dot. */
export function NavIcon({ label }: { readonly label: string }) {
  invariant(label.trim().length > 0, "a nav icon needs its entry's label");
  const glyph = ICONS[label.trim().toLowerCase()] ?? DOT;
  return (
    <svg className="ui-navrail-icon" viewBox="0 0 16 16" width="16" height="16" aria-hidden="true" focusable="false"
      fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
      {glyph}
    </svg>
  );
}

/** Whether the kit draws a dedicated icon for this label (the rest share the dot). */
export function hasNavIcon(label: string): boolean {
  return Object.hasOwn(ICONS, label.trim().toLowerCase());
}
