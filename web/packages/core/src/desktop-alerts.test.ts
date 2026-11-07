// Desktop alerts: settings survive bad storage field by field, the first attention only primes,
// every new question or approval alerts once, a burst becomes one summary, and settings filter.

import type { aether } from "@aether-ui/protocol";
import { describe, expect, test, vi } from "vitest";
import {
  ALERT_SETTINGS_KEY, DEFAULT_ALERT_SETTINGS, DesktopAlerts, alertSettingsOf, alertsOf, questionAnchor,
  readAlertSettings, writeAlertSettings, type AlertSettings,
} from "./desktop-alerts.ts";
import type { State } from "./store.ts";

const question = (agent: string, seq: number, text = "Which branch?"): aether.PendingQuestion =>
  ({ agent, agent_name: `Agent ${agent}`, asked_at: "2026-11-07T10:00:00Z", question: text, question_seq: seq });

const approval = (id: string, state = "pending"): aether.ApprovalRequest => ({
  agent_id: "a1", call_id: `call-${id}`, description: "Run the migration", id, requested_at: "2026-11-07T10:00:00Z",
  side_effect: "write", status: { state }, tool: "shell",
} as unknown as aether.ApprovalRequest);

const attention = (questions: aether.PendingQuestion[], approvals: aether.ApprovalRequest[] = []): aether.Attention =>
  ({ agents: [{ agent: "a1", agent_name: "Builder" }], approvals, last_seq: 1, questions } as unknown as aether.Attention);

const stateOf = (byConn: Record<string, aether.Attention | null>): State =>
  ({ aether: Object.fromEntries(Object.entries(byConn).map(([conn, value]) => [conn, { attention: value }])) } as unknown as State);

function alerter(settings: Partial<AlertSettings> = {}, focused = false) {
  const notify = vi.fn();
  const alerts = new DesktopAlerts({
    desktop: { notify },
    settings: async () => ({ ...DEFAULT_ALERT_SETTINGS, ...settings }),
    isFocused: () => focused,
  });
  return { alerts, notify };
}

describe("alert settings", () => {
  test("anything malformed falls back to the default, field by field", () => {
    expect(alertSettingsOf(undefined)).toEqual(DEFAULT_ALERT_SETTINGS);
    expect(alertSettingsOf([true])).toEqual(DEFAULT_ALERT_SETTINGS);
    expect(alertSettingsOf({ enabled: false, questions: "no", whileFocused: 1 }))
      .toEqual({ ...DEFAULT_ALERT_SETTINGS, enabled: false });
  });

  test("written settings read back from the same key", async () => {
    const saved = new Map<string, unknown>();
    const storage = { get: async (key: string) => saved.get(key), set: async (key: string, value: unknown) => { saved.set(key, value); } };
    const next = { ...DEFAULT_ALERT_SETTINGS, approvals: false, whileFocused: true };
    await writeAlertSettings(storage, next);
    expect([...saved.keys()]).toEqual([ALERT_SETTINGS_KEY]);
    expect(await readAlertSettings(storage)).toEqual(next);
  });
});

describe("alerts of an attention", () => {
  test("questions name their agent and anchor; only pending approvals alert", () => {
    const found = alertsOf(attention([question("a2", 7)], [approval("ap1"), approval("ap2", "approved")]));
    expect(found).toEqual([
      { key: "q:a2:7", kind: "question", title: "Agent a2 asks", body: "Which branch?", anchor: "a2:7" },
      { key: "a:ap1", kind: "approval", title: "Builder needs an approval", body: "shell: Run the migration" },
    ]);
    expect(questionAnchor({ agent: "a2", question_seq: 7 })).toBe("a2:7");
  });
});

describe("DesktopAlerts", () => {
  test("the first look only learns what waits; a later question alerts once", async () => {
    const { alerts, notify } = alerter();
    const waiting = attention([question("a1", 1)]);
    expect(alerts.fresh(stateOf({ "aether:local": waiting }))).toEqual([]);
    expect(alerts.fresh(stateOf({ "aether:local": waiting }))).toEqual([]);
    const later = attention([question("a1", 1), question("a1", 2, "Ship it?")]);
    const fresh = alerts.fresh(stateOf({ "aether:local": later }));
    expect(fresh.map((alert) => alert.key)).toEqual(["q:a1:2"]);
    expect(await alerts.deliver(fresh)).toBe(1);
    expect(notify).toHaveBeenCalledWith({ title: "Agent a1 asks", body: "Ship it?", route: "/inbox" });
    expect(alerts.fresh(stateOf({ "aether:local": attention([question("a1", 2, "Ship it?")]) }))).toEqual([]);
    expect(alerts.takeAnchor()).toBe("a1:2");
    expect(alerts.takeAnchor()).toBeUndefined();
  });

  test("a connection that goes away and comes back primes again", () => {
    const { alerts } = alerter();
    alerts.fresh(stateOf({ "aether:local": attention([]) }));
    alerts.fresh(stateOf({ "aether:local": null }));
    expect(alerts.fresh(stateOf({ "aether:local": attention([question("a1", 3)]) }))).toEqual([]);
  });

  test("more than three at once become one summary that names no question", async () => {
    const { alerts, notify } = alerter();
    alerts.fresh(stateOf({ c: attention([]) }));
    const burst = alerts.fresh(stateOf({ c: attention([1, 2, 3, 4].map((seq) => question("a1", seq))) }));
    expect(await alerts.deliver(burst)).toBe(1);
    expect(notify).toHaveBeenCalledExactlyOnceWith({
      title: "4 things need you in Aether", body: "Agents are waiting for answers and approvals.", route: "/inbox",
    });
    expect(alerts.takeAnchor()).toBeUndefined();
  });

  test("settings decide: off, focused, or the kind turned off alerts nothing", async () => {
    const found = alertsOf(attention([question("a1", 1)], [approval("ap1")]));
    for (const [settings, focused] of [[{ enabled: false }, false], [{}, true], [{ questions: false, approvals: false }, false]] as const) {
      const { alerts, notify } = alerter(settings, focused);
      expect(await alerts.deliver(found)).toBe(0);
      expect(notify).not.toHaveBeenCalled();
    }
    const focusedToo = alerter({ whileFocused: true, approvals: false }, true);
    expect(await focusedToo.alerts.deliver(found)).toBe(1);
    expect(focusedToo.notify).toHaveBeenCalledWith(expect.objectContaining({ title: "Agent a1 asks" }));
  });

  test("watch alerts on store changes and reports a failed delivery", async () => {
    let snapshot = stateOf({ c: attention([]) });
    const listeners: Array<() => void> = [];
    const store = { getSnapshot: () => snapshot, subscribe: (listener: () => void) => { listeners.push(listener); return () => {}; } };
    const report = vi.fn();
    const alerts = new DesktopAlerts({ desktop: { notify: vi.fn() }, settings: async () => { throw new Error("storage is gone"); }, isFocused: () => false });
    alerts.watch(store, report);
    snapshot = stateOf({ c: attention([question("a1", 1)]) });
    for (const listener of listeners) listener();
    await vi.waitFor(() => expect(report).toHaveBeenCalledWith("A desktop alert was not shown: storage is gone"));
  });
});
