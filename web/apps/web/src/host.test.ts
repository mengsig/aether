import { describe, expect, test } from "vitest";
import { detectHost, localStore } from "./host.ts";

function memoryStorage(): Storage {
  const data = new Map<string, string>();
  return {
    get length() { return data.size; },
    clear: () => data.clear(),
    getItem: (key) => data.get(key) ?? null,
    key: (index) => [...data.keys()][index] ?? null,
    removeItem: (key) => { data.delete(key); },
    setItem: (key, value) => { data.set(key, value); },
  };
}

describe("localStore", () => {
  test("round-trips JSON under a prefix", async () => {
    const storage = memoryStorage();
    const store = localStore(storage);
    await store.set("theme", "dark");
    expect(storage.getItem("aether.theme")).toBe('"dark"');
    expect(await store.get("theme")).toBe("dark");
    expect(await store.get("missing")).toBeUndefined();
  });

  test("names a corrupt value and refuses empty keys and undefined", async () => {
    const storage = memoryStorage();
    storage.setItem("aether.theme", "{not json");
    const store = localStore(storage);
    await expect(store.get("theme")).rejects.toThrow(/stored value of theme is not JSON/);
    await expect(store.get("")).rejects.toThrow(/not empty/);
    await expect(store.set("theme", undefined)).rejects.toThrow(/undefined/);
  });
});

describe("detectHost", () => {
  test("is the browser without a bridge and Electron with bridge version 1", () => {
    const localStorage = memoryStorage();
    expect(detectHost({ localStorage }).kind).toBe("browser");
    const desktop = { version: 1 };
    const host = detectHost({ desktop, localStorage });
    expect(host.kind).toBe("electron");
    expect(host.desktop).toBe(desktop);
    expect(() => detectHost({ desktop: { version: 2 }, localStorage })).toThrow(/version 2/);
  });
});
