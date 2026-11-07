import { describe, expect, test, vi } from "vitest";
import { QueuedStore } from "./queued.ts";

describe("the queued store", () => {
  const message = { text: "Ship it.", at: "2026-11-04T09:01:00Z", afterSeq: 2, inboxSeq: null };

  test("hands out one empty list for an unknown key, so a subscriber sees no change until a message is kept", () => {
    const store = new QueuedStore();
    expect(store.get("c/a")).toBe(store.get("c/a"));
    const heard = vi.fn();
    const stop = store.subscribe("c/a", heard);
    store.set("c/b", [message]);
    expect(heard).not.toHaveBeenCalled();
    store.set("c/a", [message]);
    expect(heard).toHaveBeenCalledTimes(1);
    expect(store.get("c/a")).toEqual([message]);
    stop();
    store.set("c/a", []);
    expect(heard).toHaveBeenCalledTimes(1);
    expect(store.get("c/a")).toBe(store.get("c/zzz"));
  });

  test("refuses a key that names nothing", () => {
    expect(() => new QueuedStore().set("", [message])).toThrow(/connection and agent/);
  });
});
