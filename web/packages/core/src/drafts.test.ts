import { describe, expect, test } from "vitest";
import { connectionDraftKey, DRAFT_STORAGE_KEY, DraftStore, isPristine, MIRROR_LIMIT_BYTES, type DraftMirror } from "./drafts.ts";

function memoryMirror(): DraftMirror & { readonly map: Map<string, string> } {
  const map = new Map<string, string>();
  return { map, getItem: (key) => map.get(key) ?? null, setItem: (key, value) => { map.set(key, value); }, removeItem: (key) => { map.delete(key); } };
}

describe("DraftStore", () => {
  test("keeps a draft per key and tells its subscribers", () => {
    const store = new DraftStore(null);
    let told = 0;
    const stop = store.subscribe("schedules/new", () => { told += 1; });
    store.set("schedules/new", { name: "nightly" });
    expect(store.get("schedules/new")).toEqual({ name: "nightly" });
    expect(store.get("memory/create")).toBeUndefined();
    expect(told).toBe(1);
    stop();
    store.clear("schedules/new");
    expect(store.get("schedules/new")).toBeUndefined();
    expect(told).toBe(1);
  });

  test("mirrors drafts to the session and reads them back", () => {
    const mirror = memoryMirror();
    const first = new DraftStore(mirror);
    first.set("controls/steer", { text: "slow down" });
    expect(JSON.parse(mirror.map.get(DRAFT_STORAGE_KEY) ?? "{}")).toEqual({ "controls/steer": { text: "slow down" } });
    const second = new DraftStore(mirror);
    expect(second.get("controls/steer")).toEqual({ text: "slow down" });
    second.clear("controls/steer");
    expect(mirror.map.has(DRAFT_STORAGE_KEY)).toBe(false);
  });

  test("an oversized draft stays in memory while the others are still mirrored", () => {
    const mirror = memoryMirror();
    const store = new DraftStore(mirror);
    store.set("controls/steer", { text: "slow down" });
    store.set("memory/import", { picked: "x".repeat(MIRROR_LIMIT_BYTES) });
    store.set("memory/create", { text: "small" });
    expect(Object.keys(JSON.parse(mirror.map.get(DRAFT_STORAGE_KEY) ?? "{}")).sort()).toEqual(["controls/steer", "memory/create"]);
    expect(store.get("memory/import")).toMatchObject({ picked: expect.stringMatching(/^x+$/) as string });
    store.clear("controls/steer");
    expect(Object.keys(JSON.parse(mirror.map.get(DRAFT_STORAGE_KEY) ?? "{}"))).toEqual(["memory/create"]);
    store.clear("memory/create");
    store.clear("memory/import");
    expect(mirror.map.has(DRAFT_STORAGE_KEY)).toBe(false);
  });

  test("a full mirror keeps the smaller drafts and never brings a cleared draft back", () => {
    const mirror = memoryMirror();
    const limited: DraftMirror = { ...mirror, setItem: (key, value) => { if (value.length > 900) throw new Error("QuotaExceededError"); mirror.map.set(key, value); } };
    const store = new DraftStore(limited);
    store.set("controls/a", { text: "a".repeat(300) });
    store.set("controls/b", { text: "b".repeat(300) });
    store.set("controls/c", { text: "c".repeat(500) });
    expect(Object.keys(JSON.parse(mirror.map.get(DRAFT_STORAGE_KEY) ?? "{}")).sort()).toEqual(["controls/a", "controls/b"]);
    expect(store.keys().sort()).toEqual(["controls/a", "controls/b", "controls/c"]);
    store.clear("controls/a");
    const again = new DraftStore(limited);
    expect(again.get("controls/a")).toBeUndefined();
    expect(again.keys().sort()).toEqual(["controls/b", "controls/c"]);
    const tiny: DraftMirror = { ...mirror, setItem: () => { throw new Error("QuotaExceededError"); } };
    const none = new DraftStore(tiny);
    none.set("controls/d", { text: "d" });
    expect(mirror.map.has(DRAFT_STORAGE_KEY)).toBe(false);
    expect(none.get("controls/d")).toEqual({ text: "d" });
  });

  test("a corrupt mirror is dropped instead of breaking the forms", () => {
    const mirror = memoryMirror();
    mirror.setItem(DRAFT_STORAGE_KEY, "{not json");
    const store = new DraftStore(mirror);
    expect(store.keys()).toEqual([]);
    mirror.setItem(DRAFT_STORAGE_KEY, JSON.stringify({ "a/b": { x: 1 }, "c/d": "not a record" }));
    expect(new DraftStore(mirror).keys()).toEqual(["a/b"]);
  });

  test("a clear of nothing tells nobody", () => {
    const store = new DraftStore(null);
    let told = 0;
    store.subscribe("queue/new-item", () => { told += 1; });
    store.clear("queue/new-item");
    expect(told).toBe(0);
  });

  test("keys name a screen and a form", () => {
    const store = new DraftStore(null);
    expect(() => store.get("no-slash")).toThrow(/<screen>\/<form>/);
    expect(() => store.set("ok/form:agt_1", {})).not.toThrow();
  });
});

describe("connectionDraftKey", () => {
  test("keeps each connection's form apart and reduces the connection id to what a key allows", () => {
    const studio = connectionDraftKey("activity", "aether:studio", "filters");
    const laptop = connectionDraftKey("activity", "aether:@laptop.local:4300", "filters");
    expect(studio).toBe("activity/aether:studio/filters");
    expect(laptop).toBe("activity/aether:_laptop.local:4300/filters");
    expect(studio).not.toBe(laptop);
    const store = new DraftStore(null);
    store.set(studio, { agent: "agt_1" });
    expect(store.get(laptop)).toBeUndefined();
    expect(connectionDraftKey("controls", "aether:b3", "update_agent/agt_1/confirmation")).toBe("controls/aether:b3/update_agent/agt_1/confirmation");
  });

  test("refuses a missing connection, a screen that is not one word, and a form with odd characters", () => {
    expect(() => connectionDraftKey("activity", "", "filters")).toThrow(/belongs to a connection/);
    expect(() => connectionDraftKey("activity/x", "aether:b3", "filters")).toThrow(/one word/);
    expect(() => connectionDraftKey("activity", "aether:b3", "fil ters")).toThrow(/plain name/);
  });
});

describe("isPristine", () => {
  test("is true only when every field matches its initial value", () => {
    expect(isPristine({ a: "", b: 1 }, { a: "", b: 1 })).toBe(true);
    expect(isPristine({ a: "x", b: 1 }, { a: "", b: 1 })).toBe(false);
    expect(isPristine({ a: "" }, { a: "", b: 1 })).toBe(false);
  });
});
