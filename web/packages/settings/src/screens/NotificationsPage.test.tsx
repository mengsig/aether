// @vitest-environment happy-dom
// Notifications: whether push is available comes from the stored get_push_config; subscribing uses
// the browser's service worker with the daemon's VAPID key and sends the subscription.

import { afterEach, describe, expect, test, vi } from "vitest";
import { connected, click, buttonNamed, type Harness } from "../kit/fixture/harness.test-util.tsx";
import { NotificationsPage } from "./NotificationsPage.tsx";
import { vapidKeyBytes } from "./push.ts";

let current: Harness | null = null;

// A 65-byte uncompressed P-256 point, base64url without padding.
const KEY = btoa(String.fromCharCode(4, ...Array.from({ length: 64 }, (_, i) => i))).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");

interface Stub { subscribe: ReturnType<typeof vi.fn>; unsubscribe: ReturnType<typeof vi.fn>; getSubscription: ReturnType<typeof vi.fn> }

function stubServiceWorker(): Stub {
  const subscription = {
    toJSON: () => ({ endpoint: "https://push.example/send/abc", keys: { p256dh: "BP-key", auth: "auth-secret" } }),
    unsubscribe: vi.fn(async () => true),
  };
  const stub: Stub = { subscribe: vi.fn(async () => subscription), unsubscribe: subscription.unsubscribe, getSubscription: vi.fn(async () => subscription) };
  Object.defineProperty(navigator, "serviceWorker", {
    configurable: true,
    value: { ready: Promise.resolve({ pushManager: { subscribe: stub.subscribe, getSubscription: stub.getSubscription } }) },
  });
  return stub;
}

afterEach(() => {
  current?.unmount();
  current = null;
  Reflect.deleteProperty(navigator, "serviceWorker");
});

const config = (subscribed: boolean, extra: object = {}) => ({ enabled: true, vapid_public_key: KEY, subscribed, kinds: ["approval", "agent_failed"], ...extra });

describe("NotificationsPage", () => {
  test("turning notifications on subscribes the browser with the VAPID key and sends the subscription", async () => {
    const stub = stubServiceWorker();
    let subscribed = false;
    const h = current = await connected({ role: "operator", script: {
      get_push_config: () => ({ tag: "push_config", data: config(subscribed) }),
      subscribe_push: () => { subscribed = true; return { tag: "ok", data: null }; },
    } });
    const root = await h.mount(<NotificationsPage conn={h.conn} />);
    expect(root.textContent).toContain("Off for this device");
    expect(root.textContent).toContain("approval, agent failed");
    await click(h, root, "Turn on notifications");
    const options = stub.subscribe.mock.calls[0]?.[0] as { userVisibleOnly: boolean; applicationServerKey: Uint8Array };
    expect(options.userVisibleOnly).toBe(true);
    expect(options.applicationServerKey).toEqual(vapidKeyBytes(KEY));
    expect(h.daemon.commands("subscribe_push")).toEqual([expect.objectContaining({ subscription: { endpoint: "https://push.example/send/abc", p256dh: "BP-key", auth: "auth-secret" } })]);
    expect(h.daemon.commands("get_push_config")).toHaveLength(2);
    expect(root.textContent).toContain("On for this device");
    expect(root.textContent).not.toContain("auth-secret");
  });

  test("when subscribed it can send a test and turn notifications off, dropping the browser subscription", async () => {
    const stub = stubServiceWorker();
    let subscribed = true;
    const h = current = await connected({ role: "operator", script: {
      get_push_config: () => ({ tag: "push_config", data: config(subscribed) }),
      test_push: () => ({ tag: "ok", data: null }),
      unsubscribe_push: () => { subscribed = false; return { tag: "ok", data: null }; },
    } });
    const root = await h.mount(<NotificationsPage conn={h.conn} />);
    await click(h, root, "Send a test");
    expect(h.daemon.commands("test_push")).toHaveLength(1);
    expect(root.textContent).toContain("Test notification sent");
    await click(h, root, "Turn off notifications");
    expect(h.daemon.commands("unsubscribe_push")).toHaveLength(0);
    await click(h, document.body, "Turn off");
    expect(h.daemon.commands("unsubscribe_push")).toHaveLength(1);
    expect(stub.unsubscribe).toHaveBeenCalledTimes(1);
    expect(root.textContent).toContain("Off for this device");
  });

  test("push the daemon has turned off is shown as unavailable", async () => {
    stubServiceWorker();
    const h = current = await connected({ role: "operator", script: { get_push_config: () => ({ tag: "push_config", data: { enabled: false } }) } });
    const root = await h.mount(<NotificationsPage conn={h.conn} />);
    expect(root.textContent).toContain("Push is not available");
    expect(root.querySelectorAll("button")).toHaveLength(0);
  });

  test("an observer is a paired device, so it may turn its own notifications on", async () => {
    stubServiceWorker();
    const h = current = await connected({ role: "observer", script: { get_push_config: () => ({ tag: "push_config", data: config(false) }) } });
    const root = await h.mount(<NotificationsPage conn={h.conn} />);
    expect(buttonNamed(root, "Turn on notifications").disabled).toBe(false);
  });

  test("the desktop's own connection is not a paired device and is told so", async () => {
    stubServiceWorker();
    const h = current = await connected({ role: "local", script: { get_push_config: () => ({ tag: "push_config", data: config(false) }) } });
    const root = await h.mount(<NotificationsPage conn={h.conn} />);
    expect(buttonNamed(root, "Turn on notifications").disabled).toBe(true);
    expect(root.querySelector(".st-reason")?.textContent).toContain("Push is for paired devices");
  });

  test("a browser without a service worker cannot subscribe and says so", async () => {
    const h = current = await connected({ role: "operator", script: { get_push_config: () => ({ tag: "push_config", data: config(false) }) } });
    const root = await h.mount(<NotificationsPage conn={h.conn} />);
    expect(buttonNamed(root, "Turn on notifications").disabled).toBe(true);
    expect(root.querySelector(".st-reason")?.textContent).toBe("This browser cannot receive push notifications");
  });

  test("a refused browser permission is reported and nothing is sent to the daemon", async () => {
    const stub = stubServiceWorker();
    stub.subscribe.mockRejectedValueOnce(new Error("Permission denied"));
    const h = current = await connected({ role: "operator", script: { get_push_config: () => ({ tag: "push_config", data: config(false) }) } });
    const root = await h.mount(<NotificationsPage conn={h.conn} />);
    await click(h, root, "Turn on notifications");
    expect(root.querySelector(".st-note")?.textContent).toBe("Permission denied");
    expect(h.daemon.commands("subscribe_push")).toHaveLength(0);
  });
});

describe("vapidKeyBytes", () => {
  test("decodes a base64url key and rejects one that is not a P-256 point", () => {
    expect(vapidKeyBytes(KEY)).toHaveLength(65);
    expect(() => vapidKeyBytes("AAAA")).toThrow(/uncompressed P-256/);
    expect(() => vapidKeyBytes("not base64!")).toThrow(/base64url/);
  });
});
