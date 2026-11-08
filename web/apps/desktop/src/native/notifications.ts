// `desktop.notify`: a calm, short native notification. Main trims the text to one plain line
// each, limits how often the page may notify, and opens the note's route on a click only when
// that route is on the deep-link allowlist.

import { BridgeRefusal } from "../security/validate.ts";
import { isAllowedRoute } from "./deep-links.ts";

export interface Note {
  readonly title: string;
  readonly body: string;
  readonly route?: string;
  readonly backgroundOnly?: boolean;
}

/** The main window's actual native state, independent of renderer focus emulation. */
export function windowInFront(window: { isVisible(): boolean; isMinimized(): boolean; isFocused(): boolean }): boolean {
  return window.isVisible() && !window.isMinimized() && window.isFocused();
}

export interface NotificationSink {
  /** Settles once the system accepted the note; rejects with the system's reason if it did not. */
  show(content: { title: string; body: string }, onClick: () => void): void | Promise<void>;
}

const SHOW_SETTLE_MS = 2_000;

/** The part of an Electron `Notification` that tells whether the system showed it. */
export interface ShownNote {
  once(event: "show", listener: () => void): unknown;
  once(event: "failed", listener: (event: unknown, error: string) => void): unknown;
  show(): void;
}

/** Resolves on `show`, rejects on the system's `failed`; a system that reports neither counts as shown. */
export function settleShown(note: ShownNote, settleMs: number = SHOW_SETTLE_MS): Promise<void> {
  if (!(settleMs > 0)) throw new Error("a notification waits a positive time for the system");
  return new Promise((resolve, reject) => {
    const timer = setTimeout(resolve, settleMs);
    note.once("show", () => { clearTimeout(timer); resolve(); });
    note.once("failed", (_event, error) => {
      clearTimeout(timer);
      reject(new Error(`The system could not show the notification: ${error}`));
    });
    note.show();
  });
}

const MAX_TITLE = 64;
const MAX_BODY = 240;
export const NOTES_PER_MINUTE = 12;

/** One plain line: control, direction and space characters become one space; cut with an ellipsis. */
export function plainLine(text: string, max: number): string {
  if (!(max > 1)) throw new Error("a line holds more than one character");
  // eslint-disable-next-line no-control-regex
  const line = text.replace(/[\u0000-\u001f\u007f-\u009f\u200e\u200f\u2028-\u202e\u2066-\u2069\s]+/gu, " ").trim();
  const characters = [...line];
  return characters.length <= max ? line : `${characters.slice(0, max - 1).join("").trimEnd()}…`;
}

export class Notifier {
  private readonly sink: NotificationSink;
  private readonly openRoute: (route: string | undefined) => void;
  private readonly now: () => number;
  private readonly recent: number[] = [];

  constructor(sink: NotificationSink, openRoute: (route: string | undefined) => void, now: () => number = Date.now) {
    this.sink = sink;
    this.openRoute = openRoute;
    this.now = now;
  }

  notify(note: Note): void | Promise<void> {
    if (note.route !== undefined && !isAllowedRoute(note.route)) throw new BridgeRefusal("A notification opens only an app route");
    const title = plainLine(note.title, MAX_TITLE);
    if (title === "") throw new BridgeRefusal("A notification has a title");
    const at = this.now();
    while (this.recent.length > 0 && at - (this.recent[0] as number) >= 60_000) this.recent.shift();
    if (this.recent.length >= NOTES_PER_MINUTE) throw new BridgeRefusal("Too many notifications; try again in a minute");
    this.recent.push(at);
    const route = note.route;
    return this.sink.show({ title, body: plainLine(note.body, MAX_BODY) }, () => this.openRoute(route));
  }
}

/** Applies the native window's focus state before handing an alert to the system. */
export async function notifyWindow(notifier: Notifier, window: Parameters<typeof windowInFront>[0], note: Note): Promise<boolean> {
  if (note.backgroundOnly === true && windowInFront(window)) return false;
  await notifier.notify(note);
  return true;
}
