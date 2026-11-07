// `desktop.notify`: a calm, short native notification. Main trims the text to one plain line
// each, limits how often the page may notify, and opens the note's route on a click only when
// that route is on the deep-link allowlist.

import { BridgeRefusal } from "../security/validate.ts";
import { isAllowedRoute } from "./deep-links.ts";

export interface Note {
  readonly title: string;
  readonly body: string;
  readonly route?: string;
}

export interface NotificationSink {
  /** Settles once the system accepted the note; rejects with the system's reason if it did not. */
  show(content: { title: string; body: string }, onClick: () => void): void | Promise<void>;
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
