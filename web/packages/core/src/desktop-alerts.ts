// Desktop alerts: a native notification when an Aether agent starts waiting on a person. It
// watches the attention core keeps synced, alerts each question or approval once, and a click
// opens the inbox on that very question. What alerts, and when, is the user's.

import type { aether, ConnId, DesktopBridge, KeyValueStore } from "@aether-ui/protocol";
import { invariant } from "./guards.ts";
import type { State } from "./store.ts";

export const ALERT_SETTINGS_KEY = "desktop.alerts";

export interface AlertSettings {
  readonly enabled: boolean;
  readonly questions: boolean;
  readonly approvals: boolean;
  /** Alert even while an Aether window has the focus. */
  readonly whileFocused: boolean;
}

export const DEFAULT_ALERT_SETTINGS: AlertSettings = { enabled: true, questions: true, approvals: true, whileFocused: false };
const SETTING_NAMES = Object.keys(DEFAULT_ALERT_SETTINGS) as Array<keyof AlertSettings>;

/** Stored settings over the defaults; anything malformed falls back to the default, field by field. */
export function alertSettingsOf(stored: unknown): AlertSettings {
  if (typeof stored !== "object" || stored === null || Array.isArray(stored)) return DEFAULT_ALERT_SETTINGS;
  const record = stored as Record<string, unknown>;
  const pick = (name: keyof AlertSettings) => (typeof record[name] === "boolean" ? record[name] : DEFAULT_ALERT_SETTINGS[name]);
  const settings = Object.fromEntries(SETTING_NAMES.map((name) => [name, pick(name)])) as unknown as AlertSettings;
  invariant(SETTING_NAMES.every((name) => typeof settings[name] === "boolean"), "every alert setting is a boolean");
  return settings;
}

export async function readAlertSettings(storage: KeyValueStore): Promise<AlertSettings> {
  return alertSettingsOf(await storage.get(ALERT_SETTINGS_KEY));
}

export async function writeAlertSettings(storage: KeyValueStore, settings: AlertSettings): Promise<void> {
  const checked = alertSettingsOf(settings);
  invariant(SETTING_NAMES.every((name) => checked[name] === settings[name]), "only whole alert settings are stored");
  await storage.set(ALERT_SETTINGS_KEY, checked);
}

export const INBOX_ROUTE = "/inbox";
/** A clicked question alert's route: `/inbox/<ticket>`, the ticket naming that alert's question. */
const TICKET_ROUTE = /^\/inbox\/([^/]+)$/;
/** More new alerts than this at once become one summary. */
export const ALERT_BURST = 3;
/** How many clicked-later question alerts are remembered; an older click opens the inbox only. */
export const MAX_TICKETS = 64;

export interface Alert {
  /** Unique within its connection while the thing waits. */
  readonly key: string;
  readonly kind: "question" | "approval";
  readonly title: string;
  readonly body: string;
  /** The `data-question` of the inbox entry a click should land on. */
  readonly anchor?: string;
}

/** The inbox entry of a question on daemon `conn`, as the inbox marks it for an alert's click to find. */
export function questionAnchor(conn: ConnId, question: Pick<aether.PendingQuestion, "agent" | "question_seq">): string {
  invariant(conn !== "", "a question belongs to a connection");
  invariant(question.agent !== "" && Number.isSafeInteger(question.question_seq), "a question has an agent and a sequence number");
  return `${conn}/${question.agent}:${question.question_seq}`;
}

/** Every alert daemon `conn`'s attention calls for: its questions and pending approvals. */
export function alertsOf(conn: ConnId, attention: aether.Attention): Alert[] {
  invariant(Array.isArray(attention.questions) && Array.isArray(attention.approvals), "attention lists questions and approvals");
  const names = new Map(attention.agents.map((agent) => [agent.agent, agent.agent_name ?? agent.agent]));
  const questions = attention.questions.map((question): Alert => {
    const anchor = questionAnchor(conn, question);
    return { key: `q:${anchor}`, kind: "question", title: `${question.agent_name ?? question.agent} asks`, body: question.question, anchor };
  });
  const approvals = attention.approvals.filter((request) => request.status.state === "pending").map((request): Alert => ({
    key: `a:${request.id}`, kind: "approval", title: `${names.get(request.agent_id) ?? request.agent_id} needs an approval`,
    body: `${request.tool}: ${request.description}`,
  }));
  return [...questions, ...approvals];
}

type Note = Parameters<DesktopBridge["notify"]>[0];

/** Shows `note` and returns whether main sent it to the system. */
export async function notifyDesktop(desktop: Pick<DesktopBridge, "notify">, note: Note): Promise<boolean> {
  invariant(note.title !== "", "a notification has a title");
  return (await desktop.notify(note)) !== false;
}

export interface AlertHost {
  readonly desktop: Pick<DesktopBridge, "notify">;
  settings(): Promise<AlertSettings>;
}

/** What a burst of alerts becomes: one notification that opens the inbox. */
function summary(count: number): { title: string; body: string; route: string } {
  invariant(count > ALERT_BURST, "only a burst is summarised");
  return { title: `${count} things need you in Aether`, body: "Agents are waiting for answers and approvals.", route: INBOX_ROUTE };
}

/** Where a route main sent lands: the hash to open, and the inbox entry a clicked alert named. */
export interface Landing {
  readonly hash: string;
  readonly anchor?: string;
}

/**
 * Turns attention changes into notifications. The first attention seen for a connection only
 * learns what already waits, so starting the app alerts nothing; later arrivals alert once.
 */
export class DesktopAlerts {
  private readonly host: AlertHost;
  private readonly seen = new Map<ConnId, { readonly attention: aether.Attention; readonly keys: ReadonlySet<string> }>();
  /** Each shown question alert's ticket, oldest first, and the inbox entry its click opens. */
  private readonly tickets = new Map<string, string>();
  /** Unique to this page's lifetime: a notification from before a reload names no ticket of ours. */
  private readonly lifetime: string;
  private issued = 0;

  constructor(host: AlertHost, lifetime: string = crypto.randomUUID()) {
    invariant(/^[A-Za-z0-9][A-Za-z0-9-]{7,63}$/.test(lifetime), "a lifetime id is a plain route segment of 8 or more characters");
    this.host = host;
    this.lifetime = lifetime;
  }

  /** The alerts that started waiting since the last look at `state`; each is returned once. */
  fresh(state: State): Alert[] {
    const found: Alert[] = [];
    for (const conn of [...this.seen.keys()]) if (state.aether[conn]?.attention == null) this.seen.delete(conn);
    for (const [conn, daemon] of Object.entries(state.aether)) {
      const attention = daemon.attention;
      const before = this.seen.get(conn);
      if (attention === null || before?.attention === attention) continue;
      const alerts = alertsOf(conn, attention);
      this.seen.set(conn, { attention, keys: new Set(alerts.map((alert) => alert.key)) });
      if (before !== undefined) found.push(...alerts.filter((alert) => !before.keys.has(alert.key)));
    }
    invariant(this.seen.size <= Object.keys(state.aether).length, "only connections with attention are remembered");
    return found;
  }

  /**
   * Shows `alerts` as the user's settings allow: one each, or one summary for a burst. Resolves
   * to how many notifications the system took; throws when main refused any of them.
   */
  async deliver(alerts: readonly Alert[]): Promise<number> {
    if (alerts.length === 0) return 0;
    const settings = await this.host.settings();
    if (!settings.enabled) return 0;
    const wanted = alerts.filter((alert) => (alert.kind === "question" ? settings.questions : settings.approvals));
    if (wanted.length === 0) return 0;
    if (wanted.length > ALERT_BURST) {
      return Number(await notifyDesktop(this.host.desktop, { ...summary(wanted.length), backgroundOnly: !settings.whileFocused }));
    }
    const sent = await Promise.allSettled(wanted.map((alert) =>
      notifyDesktop(this.host.desktop, { title: alert.title, body: alert.body, route: this.routeOf(alert), backgroundOnly: !settings.whileFocused })));
    const refused = sent.filter((result) => result.status === "rejected");
    invariant(sent.length === wanted.length, "every wanted alert was handed to the system");
    if (refused.length === 0) return sent.filter((result) => result.status === "fulfilled" && result.value).length;
    const reason: unknown = refused[0]?.reason;
    const message = reason instanceof Error ? reason.message : String(reason);
    throw new Error(refused.length === 1 ? message : `${refused.length} of ${wanted.length}: ${message}`, { cause: reason });
  }

  /** The route a click on `alert` opens: its own ticket for a question, else the inbox. */
  private routeOf(alert: Alert): string {
    if (alert.anchor === undefined) return INBOX_ROUTE;
    this.issued += 1;
    const ticket = `${this.lifetime}.${this.issued}`;
    this.tickets.set(ticket, alert.anchor);
    const oldest = this.tickets.keys().next().value;
    if (this.tickets.size > MAX_TICKETS && oldest !== undefined) this.tickets.delete(oldest);
    invariant(this.tickets.size <= MAX_TICKETS, "only the latest alerts keep their tickets");
    return `${INBOX_ROUTE}/${ticket}`;
  }

  /** Where `route` from main lands; an alert's ticket opens the inbox on its question, every click. */
  landing(route: string): Landing {
    invariant(route.startsWith("/"), `main sends app routes, not ${route}`);
    const ticket = TICKET_ROUTE.exec(route)?.[1];
    if (ticket === undefined) return { hash: `#${route}` };
    const anchor = this.tickets.get(ticket);
    return anchor === undefined ? { hash: `#${INBOX_ROUTE}` } : { hash: `#${INBOX_ROUTE}`, anchor };
  }

  /** Alerts on every committed store change until the returned function is called. */
  watch(store: { getSnapshot(): State; subscribe(listener: () => void): () => void }, report: (message: string) => void): () => void {
    this.fresh(store.getSnapshot());
    return store.subscribe(() => {
      this.deliver(this.fresh(store.getSnapshot()))
        .catch((error: unknown) => report(`A desktop alert was not shown: ${error instanceof Error ? error.message : String(error)}`));
    });
  }
}
