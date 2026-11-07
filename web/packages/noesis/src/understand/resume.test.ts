import type { aether, KeyValueStore } from "@aether-ui/protocol";
import { describe, expect, test } from "vitest";
import { findResumable, sourceKey } from "./resume.ts";
import { putInStringMap, readStringMap, STRING_MAP_CAP } from "./string-map.ts";

const session = (id: string, goal: string, updated: string): aether.NoesisSession =>
  ({ id, goal, agent: "agt_1", mode: "teach", phase: "probe", created_at: updated, updated_at: updated }) as aether.NoesisSession;

function memoryStore(initial: Record<string, unknown> = {}): KeyValueStore & { readonly data: Map<string, unknown> } {
  const data = new Map<string, unknown>(Object.entries(initial));
  return { data, get: async (key) => data.get(key), set: async (key, value) => { data.set(key, value); } };
}

describe("the source key", () => {
  test("names the same pull request, or the same repository and day, the same way", () => {
    expect(sourceKey({ kind: "pull_request", conn: "archon:a", id: "github.com/acme/forge#212" })).toBe("pull_request|archon:a|github.com/acme/forge#212");
    const base = { kind: "repository", conn: "archon:a", id: "repo_forge" } as const;
    expect(sourceKey({ ...base, since: "2026-11-01" })).toBe("repository|archon:a|repo_forge|2026-11-01");
    expect(sourceKey({ ...base, since: "2026-11-01" })).not.toBe(sourceKey({ ...base, since: "2026-11-02" }));
  });
});

describe("finding the session to resume", () => {
  const sessions = [session("ns_1", "Understand #212", "2026-11-02T10:00:00Z"), session("ns_2", "Something else", "2026-11-03T10:00:00Z"), session("ns_3", "Understand #212", "2026-11-04T10:00:00Z")];

  test("the remembered session wins while it still exists", () => {
    expect(findResumable(sessions, "ns_1", "Understand #212")?.id).toBe("ns_1");
  });

  test("a remembered session that is gone falls back to the newest with the same goal", () => {
    expect(findResumable(sessions, "ns_gone", "Understand #212")?.id).toBe("ns_3");
    expect(findResumable(sessions, undefined, " Understand #212 ")?.id).toBe("ns_3");
  });

  test("nothing matches a goal no session has", () => {
    expect(findResumable(sessions, undefined, "Understand #999")).toBeNull();
  });
});

describe("the remembered strings", () => {
  test("a missing value is an empty map and entries that are not strings are dropped", async () => {
    expect(await readStringMap(memoryStore(), "k")).toEqual({});
    expect(await readStringMap(memoryStore({ k: { a: "x", b: 3, c: null } }), "k")).toEqual({ a: "x" });
  });

  test("a value that is not a map is refused, not guessed at", async () => {
    await expect(readStringMap(memoryStore({ k: "text" }), "k")).rejects.toThrow("k holds a map");
    await expect(readStringMap(memoryStore({ k: ["x"] }), "k")).rejects.toThrow("k holds a map");
  });

  test("setting an entry again makes it the newest, and the oldest are dropped past the cap", async () => {
    const store = memoryStore();
    for (let index = 0; index < STRING_MAP_CAP + 5; index += 1) await putInStringMap(store, "k", `key${index}`, `v${index}`);
    await putInStringMap(store, "k", "key100", "again");
    const map = await readStringMap(store, "k");
    expect(Object.keys(map)).toHaveLength(STRING_MAP_CAP);
    expect(Object.keys(map).at(-1)).toBe("key100");
    expect(map.key0).toBeUndefined();
    expect(map.key100).toBe("again");
  });
});
