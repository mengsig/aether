// The native surfaces main owns: deep links checked against the route allowlist, calm
// rate-limited notifications, and single-use, short-lived folder picker tokens.

import { EventEmitter } from "node:events";
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { DeepLinkRouter, isAllowedRoute, linksInArgv, parseDeepLink, type RouteTarget } from "../../src/native/deep-links.ts";
import { windowTarget } from "../../src/native/window-target.ts";
import { FolderTokens, TOKEN_TTL_MS } from "../../src/native/folder-tokens.ts";
import { NOTES_PER_MINUTE, Notifier, notifyWindow, plainLine, settleShown, windowInFront } from "../../src/native/notifications.ts";
import { BridgeRefusal } from "../../src/security/validate.ts";

test("native focus accounts for hidden and minimized windows", () => {
  const state = (visible: boolean, minimized: boolean, focused: boolean) => ({ isVisible: () => visible, isMinimized: () => minimized, isFocused: () => focused });
  assert.equal(windowInFront(state(true, false, true)), true);
  assert.equal(windowInFront(state(false, false, true)), false);
  assert.equal(windowInFront(state(true, true, true)), false);
  assert.equal(windowInFront(state(true, false, false)), false);
});

test("background-only alerts use native window state and test alerts always send", async () => {
  const shown: string[] = [];
  const notifier = new Notifier({ show: (note) => { shown.push(note.title); } }, () => {});
  let focused = true;
  const window = { isVisible: () => true, isMinimized: () => false, isFocused: () => focused };
  assert.equal(await notifyWindow(notifier, window, { title: "Question", body: "", backgroundOnly: true }), false);
  assert.deepEqual(shown, []);
  assert.equal(await notifyWindow(notifier, window, { title: "Test", body: "" }), true);
  focused = false;
  assert.equal(await notifyWindow(notifier, window, { title: "Question", body: "", backgroundOnly: true }), true);
  assert.deepEqual(shown, ["Test", "Question"]);
});

test("a deep link opens only an allowlisted route", () => {
  const good: ReadonlyArray<readonly [string, string]> = [
    ["aether-app://open?route=/fleet", "/fleet"],
    ["aether-app://open?route=%2Ffleet%2Ftopology", "/fleet/topology"],
    ["aether-app://open/?route=/archon/queue", "/archon/queue"],
    ["AETHER-APP://open?route=/archon/items/qi_01ABC", "/archon/items/qi_01ABC"],
    ["aether-app://open?route=/archon/items/qi_1/runs/run-2", "/archon/items/qi_1/runs/run-2"],
    ["aether-app://open?route=/agents/local:1f2e/agt_01X", "/agents/local:1f2e/agt_01X"],
    ["aether-app://open?route=/inbox/0f8c2b9e-4b1d-4c55-9a2e-5d7f3c1a8b60.12", "/inbox/0f8c2b9e-4b1d-4c55-9a2e-5d7f3c1a8b60.12"],
  ];
  for (const [link, route] of good) assert.equal(parseDeepLink(link), route, link);
  const bad = [
    "aether-app://open?route=/pair/aether:local", "aether-app://open?route=/settings", "aether-app://open?route=fleet",
    "aether-app://open?route=/fleet/", "aether-app://open?route=/fleet?x=1", "aether-app://open?route=/fleet#x",
    "aether-app://open?route=/archon/items/../queue", "aether-app://open?route=/archon/items/%2e%2e",
    "aether-app://open?route=/archon/items/a%2Fb", "aether-app://open?route=/agents/x/y/z", "aether-app://open?route=/inbox/a/b",
    "aether-app://open?route=/fleet&route=/archon/queue", "aether-app://open?route=/fleet&x=1", "aether-app://open",
    "aether-app://open/fleet?route=/fleet", "aether-app://other?route=/fleet", "aether-app://u:p@open?route=/fleet",
    "aether-app://open:1?route=/fleet", "aether-app://open?route=/fleet#frag", "https://open?route=/fleet",
    "javascript:alert(1)", "", `aether-app://open?route=/archon/items/${"a".repeat(2000)}`,
  ];
  for (const link of bad) assert.equal(parseDeepLink(link), undefined, link);
  assert.equal(isAllowedRoute("/archon/items/qi 1"), false);
  assert.deepEqual(linksInArgv(["/opt/aether", "--flag", "aether-app://open?route=/fleet", "https://x"]), ["aether-app://open?route=/fleet"]);
});

function recordingTarget(ready: { value: boolean }): RouteTarget & { shown: string[]; focused: number } {
  const target = {
    shown: [] as string[],
    focused: 0,
    isReady: () => ready.value,
    focus: () => { target.focused += 1; },
    show: (route: string) => { target.shown.push(route); },
  };
  return target;
}

test("the router holds the latest link until the window has loaded, then delivers each", () => {
  const router = new DeepLinkRouter();
  assert.equal(router.acceptLink("aether-app://open?route=/fleet"), true);
  router.acceptArgv(["electron", "aether-app://open?route=/archon/queue", "aether-app://open?route=/pair/x"]);
  assert.equal(router.acceptLink("aether-app://open?route=/settings"), false);
  const ready = { value: false };
  const target = recordingTarget(ready);
  assert.throws(() => router.attach(target), /once it has loaded/);
  ready.value = true;
  router.attach(target);
  assert.deepEqual(target.shown, ["/archon/queue"], "only the latest waiting link opens");
  router.acceptLink("aether-app://open?route=/fleet/topology");
  router.focus();
  assert.deepEqual(target.shown, ["/archon/queue", "/fleet/topology"]);
  assert.equal(target.focused, 1);
  assert.throws(() => router.open("/pair/x"), /not an allowed route/);
});

test("notifications are short plain lines, rate limited, and open only allowed routes", () => {
  const shown: Array<{ title: string; body: string; click: () => void }> = [];
  const opened: Array<string | undefined> = [];
  let now = 1_000_000;
  const notifier = new Notifier({ show: (content, click) => { shown.push({ ...content, click }); } }, (route) => opened.push(route), () => now);
  notifier.notify({ title: " Review\nready‮ ", body: `Item\u0007 qi_1\r\n${"x".repeat(400)}`, route: "/archon/items/qi_1" });
  assert.equal(shown[0]?.title, "Review ready");
  assert.equal([...(shown[0]?.body ?? "")].length, 240);
  assert.ok(shown[0]?.body.startsWith("Item qi_1 x") && shown[0].body.endsWith("…"));
  shown[0]?.click();
  assert.deepEqual(opened, ["/archon/items/qi_1"]);
  assert.throws(() => notifier.notify({ title: "x", body: "", route: "/pair/aether:local" }), BridgeRefusal);
  assert.throws(() => notifier.notify({ title: "\n\t", body: "" }), BridgeRefusal);
  for (let sent = 1; sent < NOTES_PER_MINUTE; sent += 1) notifier.notify({ title: `n${sent}`, body: "" });
  assert.throws(() => notifier.notify({ title: "one too many", body: "" }), /Too many notifications/);
  assert.equal(shown.length, NOTES_PER_MINUTE);
  now += 60_000;
  notifier.notify({ title: "a minute later", body: "" });
  shown.at(-1)?.click();
  assert.deepEqual(opened, ["/archon/items/qi_1", undefined]);
  assert.equal(plainLine("abcdef", 4), "abc…");
});

test("a notification the system refuses to show rejects to the caller instead of reporting success", async () => {
  const failing = { show: () => Promise.reject(new Error("The system could not show the notification: denied")) };
  const notifier = new Notifier(failing, () => undefined);
  await assert.rejects(Promise.resolve(notifier.notify({ title: "Question", body: "ready?" })), /could not show the notification: denied/);
  const accepting = new Notifier({ show: () => Promise.resolve() }, () => undefined);
  assert.equal(await accepting.notify({ title: "Question", body: "ready?" }), undefined);
});

test("settleShown follows the system's own show, failed and silence", async () => {
  const note = (emit: (events: EventEmitter) => void) => {
    const events = new EventEmitter();
    return { once: events.once.bind(events), show: () => { setImmediate(() => emit(events)); } } as Parameters<typeof settleShown>[0];
  };
  assert.equal(await settleShown(note((events) => events.emit("show"))), undefined);
  await assert.rejects(settleShown(note((events) => events.emit("failed", {}, "denied"))), /could not show the notification: denied/);
  assert.equal(await settleShown(note(() => undefined), 20), undefined, "a system that says nothing counts as shown");
  assert.throws(() => settleShown(note(() => undefined), 0), /positive time/);
});

test("a folder token is opaque, single use, bound to its purpose and short lived", () => {
  const dir = mkdtempSync(path.join(os.tmpdir(), "e2-folders-"));
  const chosen = path.join(dir, "project");
  mkdirSync(chosen);
  let now = 5_000;
  const tokens = new FolderTokens(() => now);
  try {
    const token = tokens.issue(chosen, "workspace");
    assert.match(token, /^[A-Za-z0-9_-]{43}$/);
    assert.ok(!token.includes(dir) && !token.includes("project"), "a token reveals nothing of the path");
    assert.notEqual(tokens.issue(chosen, "workspace"), token);
    assert.equal(tokens.redeem(token, "workspace"), chosen);
    assert.throws(() => tokens.redeem(token, "workspace"), /already used/, "a token is used once");

    const wrongPurpose = tokens.issue(chosen, "workspace");
    assert.throws(() => tokens.redeem(wrongPurpose, "repository"), /for something else/);
    assert.throws(() => tokens.redeem(wrongPurpose, "workspace"), /already used/, "a refused attempt uses it up");

    const late = tokens.issue(chosen, "workspace");
    now += TOKEN_TTL_MS;
    assert.throws(() => tokens.redeem(late, "workspace"), /expired/);

    const gone = tokens.issue(path.join(dir, "missing"), "workspace");
    assert.throws(() => tokens.redeem(gone, "workspace"), /gone/);
    assert.throws(() => tokens.redeem("made-up", "workspace"), /never made/);
    assert.throws(() => tokens.issue("relative/dir", "workspace"), /absolute/);
    assert.throws(() => tokens.issue(chosen, "Work Space"), BridgeRefusal);

    for (let index = 0; index < 40; index += 1) tokens.issue(chosen, "workspace");
    assert.equal(tokens.live, 16, "live grants are capped");
    now += TOKEN_TTL_MS;
    assert.equal(tokens.live, 0, "expired grants are dropped");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("notification clicks restore and show the app before opening the question route", () => {
  const calls: string[] = [];
  let minimized = true;
  let visible = false;
  const window = {
    isDestroyed: () => false,
    isMinimized: () => minimized,
    isVisible: () => visible,
    restore: () => { calls.push("restore"); minimized = false; },
    hide: () => { calls.push("hide"); visible = false; },
    show: () => { calls.push("show"); visible = true; },
    focus: () => { calls.push("focus"); },
    webContents: {
      isCrashed: () => false,
      send: (channel: string, route: string) => { calls.push(`${channel}:${route}`); },
    },
  };
  const router = new DeepLinkRouter();
  router.attach(windowTarget(window));
  let click: () => void = () => { throw new Error("notification was not shown"); };
  const notifier = new Notifier({ show: (_content, onClick) => { click = onClick; } },
    (route) => { if (route === undefined) router.focus(); else router.open(route); });
  notifier.notify({ title: "Question", body: "Answer?", route: "/inbox/ticket.1" });
  click();
  assert.deepEqual(calls, ["restore", "show", "focus", "desktop:deep-link:/inbox/ticket.1"]);
  calls.length = 0;
  notifier.notify({ title: "Alert", body: "" });
  click();
  assert.deepEqual(calls, ["hide", "show", "focus"]);
});
