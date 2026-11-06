import { describe, expect, test } from "vitest";
import { DRAFT_STORAGE_KEY, DraftStore, isPristine, MIRROR_LIMIT_BYTES, type DraftMirror } from "./drafts.ts";

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

describe("isPristine", () => {
  test("is true only when every field matches its initial value", () => {
    expect(isPristine({ a: "", b: 1 }, { a: "", b: 1 })).toBe(true);
    expect(isPristine({ a: "x", b: 1 }, { a: "", b: 1 })).toBe(false);
    expect(isPristine({ a: "" }, { a: "", b: 1 })).toBe(false);
  });
});
