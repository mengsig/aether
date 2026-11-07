// Desktop alerts: a native notification when an Aether agent starts waiting on a person. It
// watches the attention core keeps synced, alerts each question or approval once, and opens the
// inbox on a click. What alerts, and whether it does while the app is in front, is the user's.

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
/** More new alerts than this at once become one summary. */
export const ALERT_BURST = 3;

export interface Alert {
  /** Unique within its connection while the thing waits. */
  readonly key: string;
  readonly kind: "question" | "approval";
  readonly title: string;
  readonly body: string;
  /** The `data-question` of the inbox entry a click should land on. */
  readonly anchor?: string;
}

/** The inbox entry of a question, as the inbox marks it for an alert's click to find. */
export function questionAnchor(question: Pick<aether.PendingQuestion, "agent" | "question_seq">): string {
  invariant(question.agent !== "" && Number.isSafeInteger(question.question_seq), "a question has an agent and a sequence number");
  return `${question.agent}:${question.question_seq}`;
}

/** Every alert one daemon's attention calls for: its questions and pending approvals. */
export function alertsOf(attention: aether.Attention): Alert[] {
  invariant(Array.isArray(attention.questions) && Array.isArray(attention.approvals), "attention lists questions and approvals");
  const names = new Map(attention.agents.map((agent) => [agent.agent, agent.agent_name ?? agent.agent]));
  const questions = attention.questions.map((question): Alert => {
    const anchor = questionAnchor(question);
    return { key: `q:${anchor}`, kind: "question", title: `${question.agent_name ?? question.agent} asks`, body: question.question, anchor };
  });
  const approvals = attention.approvals.filter((request) => request.status.state === "pending").map((request): Alert => ({
    key: `a:${request.id}`, kind: "approval", title: `${names.get(request.agent_id) ?? request.agent_id} needs an approval`,
    body: `${request.tool}: ${request.description}`,
  }));
  return [...questions, ...approvals];
}

export interface AlertHost {
  readonly desktop: Pick<DesktopBridge, "notify">;
  settings(): Promise<AlertSettings>;
  /** Whether an Aether window has the focus now. */
  isFocused(): boolean;
}

/** What a burst of alerts becomes: one notification that opens the inbox. */
function summary(count: number): { title: string; body: string; route: string } {
  invariant(count > ALERT_BURST, "only a burst is summarised");
  return { title: `${count} things need you in Aether`, body: "Agents are waiting for answers and approvals.", route: INBOX_ROUTE };
}

/**
 * Turns attention changes into notifications. The first attention seen for a connection only
 * learns what already waits, so starting the app alerts nothing; later arrivals alert once.
 */
export class DesktopAlerts {
  private readonly host: AlertHost;
  private readonly seen = new Map<ConnId, { readonly attention: aether.Attention; readonly keys: ReadonlySet<string> }>();
  private anchor: string | undefined;

  constructor(host: AlertHost) {
    this.host = host;
  }

  /** The alerts that started waiting since the last look at `state`; each is returned once. */
  fresh(state: State): Alert[] {
    const found: Alert[] = [];
    for (const conn of [...this.seen.keys()]) if (state.aether[conn]?.attention == null) this.seen.delete(conn);
    for (const [conn, daemon] of Object.entries(state.aether)) {
      const attention = daemon.attention;
      const before = this.seen.get(conn);
      if (attention === null || before?.attention === attention) continue;
      const alerts = alertsOf(attention);
      this.seen.set(conn, { attention, keys: new Set(alerts.map((alert) => alert.key)) });
      if (before !== undefined) found.push(...alerts.filter((alert) => !before.keys.has(alert.key)));
    }
    invariant(this.seen.size <= Object.keys(state.aether).length, "only connections with attention are remembered");
    return found;
  }

  /** Shows `alerts` as the user's settings allow: one each, or one summary for a burst. */
  async deliver(alerts: readonly Alert[]): Promise<number> {
    if (alerts.length === 0) return 0;
    const settings = await this.host.settings();
    if (!settings.enabled || (!settings.whileFocused && this.host.isFocused())) return 0;
    const wanted = alerts.filter((alert) => (alert.kind === "question" ? settings.questions : settings.approvals));
    if (wanted.length === 0) return 0;
    if (wanted.length > ALERT_BURST) {
      this.anchor = undefined;
      this.host.desktop.notify(summary(wanted.length));
      return 1;
    }
    for (const alert of wanted) this.host.desktop.notify({ title: alert.title, body: alert.body, route: INBOX_ROUTE });
    this.anchor = wanted.findLast((alert) => alert.anchor !== undefined)?.anchor;
    return wanted.length;
  }

  /** The inbox entry the last alert named, handed once to the click that opens the inbox. */
  takeAnchor(): string | undefined {
    const anchor = this.anchor;
    this.anchor = undefined;
    return anchor;
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
