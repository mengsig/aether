import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createApp } from "./app.ts";
import type { Json } from "./guards.ts";
import { FakeDaemon, type Answer } from "./fixtures/fake-daemon.ts";
import F from "./fixtures/archon-frames.json";

const CONN = "archon:host";

/** An archond speaking protocol 24 whose GitHub watcher read fails, as it does in shared mode. */
function archond(command: Json): Answer {
  switch (command.cmd) {
    case "hello": return { ok: F.welcome_24 };
    case "subscribe_changes": return { ok: F.subscribed_changes, then: command.after === null ? F.changes : [] };
    case "task_graph": return { ok: F.task_graph };
    case "list_item_summaries": return { ok: F.summaries_24 };
    case "feed_history": return { ok: F.feed_history };
    case "subscribe_feed": return { ok: F.subscribed_feed };
    case "github_watcher": return { err: { code: "invalid_argument", message: "the GitHub watcher needs Archon's own Aether" } };
    case "repo_list": return { ok: { reply: "repos", data: [] } };
    default: throw new Error(`unscripted ${String(command.cmd)}`);
  }
}

beforeEach(() => { vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval"] }); });
afterEach(() => { vi.useRealTimers(); });

async function ready() {
  const daemon = new FakeDaemon(archond, "ws-archon");
  const app = createApp({ kind: "browser", storage: { get: async () => undefined, set: async () => {} } },
    { scheduler: (flush) => flush(), endpoints: [{ daemon: "archon", transport: daemon.transport, target: "wss://host/archon/ws", label: "host" }] });
  await app.start();
  for (let spent = 0; app.store.get().connections[CONN]?.state !== "ready"; spent += 10) {
    if (spent > 5_000) throw new Error("the fake archond never became ready");
    await vi.advanceTimersByTimeAsync(10);
  }
  return { app, daemon };
}

describe("a failing reread", () => {
  it("records the error on the read for its screen and raises no report", async () => {
    const { app } = await ready();
    await app.reread(CONN, "github:watcher", { cmd: "github_watcher" }, ["github_watcher"]);
    expect(app.store.get().reads[CONN]?.["github:watcher"]).toMatchObject({ error: expect.stringContaining("Archon's own Aether"), value: null });
    expect(app.store.get().reports).toEqual([]);
    app.stop();
  });

  it("leaves a user's own failing send to report, as before", async () => {
    const { app } = await ready();
    await expect(app.send(CONN, { cmd: "github_watcher" })).rejects.toThrow(/Archon's own Aether/);
    expect(app.store.get().reports).toEqual([expect.objectContaining({ code: "invalid_argument", conn: CONN })]);
    app.stop();
  });
});
