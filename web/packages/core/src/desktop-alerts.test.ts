// Desktop alerts: settings survive bad storage field by field, the first attention only primes,
// every new question or approval alerts once, each click finds its own question, a burst becomes
// one summary, settings filter, and a notification main refuses is reported, not counted.

import type { aether } from "@aether-ui/protocol";
import { describe, expect, test, vi } from "vitest";
import {
  ALERT_SETTINGS_KEY, DEFAULT_ALERT_SETTINGS, DesktopAlerts, MAX_TICKETS, alertSettingsOf, notifyDesktop, alertsOf, questionAnchor,
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
  const notify = vi.fn<(note: { title: string; body: string; route?: string }) => Promise<void>>(async () => {});
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
    const found = alertsOf("c1", attention([question("a2", 7)], [approval("ap1"), approval("ap2", "approved")]));
    expect(found).toEqual([
      { key: "q:c1/a2:7", kind: "question", title: "Agent a2 asks", body: "Which branch?", anchor: "c1/a2:7" },
      { key: "a:ap1", kind: "approval", title: "Builder needs an approval", body: "shell: Run the migration" },
    ]);
    expect(questionAnchor("c2", { agent: "a2", question_seq: 7 }), "the same question on another daemon is another entry").toBe("c2/a2:7");
    expect(() => questionAnchor("", { agent: "a2", question_seq: 7 })).toThrow();
  });
});

describe("notifyDesktop", () => {
  test("settles with main's answer, or at once when the bridge returns none", async () => {
    await expect(notifyDesktop({ notify: () => {} }, { title: "t", body: "" })).resolves.toBeUndefined();
    await expect(notifyDesktop({ notify: async () => { throw new Error("Too many notifications"); } }, { title: "t", body: "" }))
      .rejects.toThrow("Too many notifications");
    await expect(notifyDesktop({ notify: () => {} }, { title: "", body: "" })).rejects.toThrow(/title/);
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
    expect(fresh.map((alert) => alert.key)).toEqual(["q:aether:local/a1:2"]);
    expect(await alerts.deliver(fresh)).toBe(1);
    const route = notify.mock.calls[0]?.[0].route ?? "";
    expect(notify).toHaveBeenCalledWith({ title: "Agent a1 asks", body: "Ship it?", route });
    expect(route).toMatch(/^\/inbox\/[0-9a-f-]{36}\.1$/);
    expect(alerts.fresh(stateOf({ "aether:local": attention([question("a1", 2, "Ship it?")]) }))).toEqual([]);
    expect(alerts.landing(route)).toEqual({ hash: "#/inbox", anchor: "aether:local/a1:2" });
    expect(alerts.landing(route), "a second click on the same alert finds it again").toEqual({ hash: "#/inbox", anchor: "aether:local/a1:2" });
  });

  test("a notification from before a reload never opens a question asked after it", async () => {
    const before = alerter();
    before.alerts.fresh(stateOf({ c: attention([]) }));
    await before.alerts.deliver(before.alerts.fresh(stateOf({ c: attention([question("a1", 1, "Old?")]) })));
    const after = alerter();
    after.alerts.fresh(stateOf({ c: attention([]) }));
    await after.alerts.deliver(after.alerts.fresh(stateOf({ c: attention([question("a1", 2, "New?")]) })));
    const old = before.notify.mock.calls[0]?.[0].route ?? "";
    const fresh = after.notify.mock.calls[0]?.[0].route ?? "";
    expect(old).not.toBe(fresh);
    expect(after.alerts.landing(old), "the old click opens the inbox only").toEqual({ hash: "#/inbox" });
    expect(after.alerts.landing(fresh)).toEqual({ hash: "#/inbox", anchor: "c/a1:2" });
    expect(() => new DesktopAlerts({ desktop: { notify: () => {} }, settings: async () => DEFAULT_ALERT_SETTINGS, isFocused: () => false }, "x/y"))
      .toThrow(/lifetime/);
  });

  test("each alert's click opens its own question, on its own daemon", async () => {
    const { alerts, notify } = alerter();
    alerts.fresh(stateOf({ c1: attention([]), c2: attention([]) }));
    const fresh = alerts.fresh(stateOf({ c1: attention([question("a1", 1, "First?"), question("a1", 2, "Second?")]), c2: attention([question("a1", 1, "Other?")]) }));
    expect(await alerts.deliver(fresh)).toBe(3);
    const routes = notify.mock.calls.map(([note]) => note.route ?? "");
    expect(new Set(routes).size, "every alert has a route of its own").toBe(3);
    expect(routes.map((route) => alerts.landing(route).anchor)).toEqual(["c1/a1:1", "c1/a1:2", "c2/a1:1"]);
  });

  test("routes that are not an alert's ticket land where they say, or on the inbox", () => {
    const { alerts } = alerter();
    expect(alerts.landing("/fleet")).toEqual({ hash: "#/fleet" });
    expect(alerts.landing("/inbox")).toEqual({ hash: "#/inbox" });
    expect(alerts.landing("/inbox/0000aaaa.9"), "an unknown or forgotten ticket opens the inbox only").toEqual({ hash: "#/inbox" });
    expect(() => alerts.landing("inbox")).toThrow();
  });

  test("only the latest alerts keep their tickets", async () => {
    const { alerts, notify } = alerter({ whileFocused: true });
    alerts.fresh(stateOf({ c: attention([]) }));
    for (let seq = 1; seq <= MAX_TICKETS + 1; seq += 1) {
      await alerts.deliver(alerts.fresh(stateOf({ c: attention([question("a1", seq)]) })));
    }
    const routes = notify.mock.calls.map(([note]) => note.route ?? "");
    expect(routes).toHaveLength(MAX_TICKETS + 1);
    expect(alerts.landing(routes[0] ?? "")).toEqual({ hash: "#/inbox" });
    expect(alerts.landing(routes.at(-1) ?? "")).toEqual({ hash: "#/inbox", anchor: `c/a1:${MAX_TICKETS + 1}` });
  });

  test("a notification main refuses fails the delivery with main's reason", async () => {
    const notify = vi.fn(async (note: { title: string }) => { if (note.title === "Agent a2 asks") throw new Error("Too many notifications; try again in a minute"); });
    const alerts = new DesktopAlerts({ desktop: { notify }, settings: async () => DEFAULT_ALERT_SETTINGS, isFocused: () => false });
    const found = alertsOf("c", attention([question("a1", 1), question("a2", 1)]));
    await expect(alerts.deliver(found)).rejects.toThrow(/^Too many notifications; try again in a minute$/);
    expect(notify, "the other alert was still shown").toHaveBeenCalledTimes(2);
    await expect(alerts.deliver(alertsOf("c", attention([question("a2", 2), question("a2", 3)])))).rejects.toThrow(/^2 of 2: Too many/);
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
    expect(alerts.landing("/inbox")).toEqual({ hash: "#/inbox" });
  });

  test("settings decide: off, focused, or the kind turned off alerts nothing", async () => {
    const found = alertsOf("c", attention([question("a1", 1)], [approval("ap1")]));
    for (const [settings, focused] of [[{ enabled: false }, false], [{}, true], [{ questions: false, approvals: false }, false]] as const) {
      const { alerts, notify } = alerter(settings, focused);
      expect(await alerts.deliver(found)).toBe(0);
      expect(notify).not.toHaveBeenCalled();
    }
    const focusedToo = alerter({ whileFocused: true, approvals: false }, true);
    expect(await focusedToo.alerts.deliver(found)).toBe(1);
    expect(focusedToo.notify).toHaveBeenCalledWith(expect.objectContaining({ title: "Agent a1 asks" }));
  });

  test("watch reports a notification main refused", async () => {
    let snapshot = stateOf({ c: attention([]) });
    const listeners: Array<() => void> = [];
    const store = { getSnapshot: () => snapshot, subscribe: (listener: () => void) => { listeners.push(listener); return () => {}; } };
    const report = vi.fn();
    const notify = vi.fn(async () => { throw new Error("This system shows no notifications"); });
    new DesktopAlerts({ desktop: { notify }, settings: async () => DEFAULT_ALERT_SETTINGS, isFocused: () => false }).watch(store, report);
    snapshot = stateOf({ c: attention([question("a1", 1)]) });
    for (const listener of listeners) listener();
    await vi.waitFor(() => expect(report).toHaveBeenCalledWith("A desktop alert was not shown: This system shows no notifications"));
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
