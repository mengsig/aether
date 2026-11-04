import type { aether } from "@aether-ui/protocol";
import { describe, expect, it } from "vitest";
import { applyDelta, applyEvent, endRecovery } from "./aether-reduce.ts";
import type { Json } from "./guards.ts";
import { EMPTY_AETHER, type AetherConnState } from "./store.ts";

const AGENT = "agt_01J00000000000000000000001";
const INV = "inv_01J00000000000000000000001";

const delta = (order: number, body: Json): Json => ({ frame: "delta", sub: 2, agent: AGENT, invocation: INV, invocation_seq: order, delta: body });
const text = (order: number, value: string) => delta(order, { delta: "text", text: value });
const commit = (at: number, role = "assistant"): aether.Event => ({
  seq: at, id: `evt_${at}`, at: "2026-11-04T03:00:00Z", actor: { actor: "system" }, agent_id: AGENT,
  kind: { type: "agent.message", role, invocation: role === "assistant" ? INV : null, content: [{ type: "text", text: "Hello" }] },
} as unknown as aether.Event);

function feed(state: AetherConnState, frames: readonly Json[]): AetherConnState {
  return frames.reduce((s, frame) => applyDelta(s, frame).state, state);
}

describe("the streamed tail of an invocation", () => {
  it("is emptied by its committed message and drops a late delta after it", () => {
    let state = feed(EMPTY_AETHER, [text(1, "Hel"), text(2, "lo")]);
    expect(state.streams[INV]?.text).toBe("Hello");
    state = applyEvent(state, commit(10));
    expect(state.streams[INV]).toMatchObject({ text: "", committed: true });
    const late = applyDelta(state, text(3, " again"));
    expect(late.outcome).toBe("ignored");
    expect(late.state.streams[INV]?.text).toBe("");
  });

  it("leaves a tombstone when the commit arrives before any delta", () => {
    const state = feed(applyEvent(EMPTY_AETHER, commit(10)), [text(1, "Hel"), text(2, "lo")]);
    expect(state.streams[INV]).toMatchObject({ text: "", committed: true });
  });

  it("ignores a user message, which commits no invocation", () => {
    const state = applyEvent(feed(EMPTY_AETHER, [text(1, "Hi")]), commit(10, "user"));
    expect(state.streams[INV]).toMatchObject({ text: "Hi", committed: false });
  });

  it("collects tool input by tool_use_id", () => {
    const tool = (order: number, id: string, chunk: string) => delta(order, { delta: "tool_input", name: "bash", partial_json: chunk, tool_use_id: id });
    const state = feed(EMPTY_AETHER, [tool(1, "toolu_a", ""), tool(2, "toolu_b", "{\"x\""), tool(3, "toolu_a", "{\"cmd\":"), tool(4, "toolu_a", "\"ls\"}")]);
    expect(state.streams[INV]?.tools).toEqual([
      { id: "toolu_a", name: "bash", input: "{\"cmd\":\"ls\"}" },
      { id: "toolu_b", name: "bash", input: "{\"x\"" },
    ]);
  });

  it("keeps a bounded number of tombstones when the daemon completes an invocation before committing it", () => {
    let state = feed(EMPTY_AETHER, [delta(1, { delta: "text", text: "live" })]);
    for (let n = 1; n <= 100; n += 1) {
      const invocation = `inv_done_${n}`;
      const event = (at: number, kind: Json) => ({ seq: at, id: `evt_${at}`, at: "2026-11-04T03:00:00Z", actor: { actor: "system" }, agent_id: AGENT, kind } as unknown as aether.Event);
      state = applyEvent(state, event(n * 2, { type: "invocation.completed", invocation }));
      state = applyEvent(state, event(n * 2 + 1, { type: "agent.message", role: "assistant", invocation, content: [] }));
    }
    const streams = Object.values(state.streams);
    expect(streams.filter((stream) => stream.committed)).toHaveLength(64);
    expect(state.streams[INV]).toMatchObject({ text: "live", committed: false });
    expect(state.streams.inv_done_1).toBeUndefined();
    const late = applyDelta(state, { frame: "delta", sub: 2, agent: AGENT, invocation: "inv_done_100", invocation_seq: 1, delta: { delta: "text", text: "late" } });
    expect(late.outcome).toBe("ignored");
  });

  it("marks text resumed after a gap as partial", () => {
    let state = feed(EMPTY_AETHER, [text(1, "Hello"), text(3, " lost")]);
    expect(state.streams[INV]).toMatchObject({ text: "", recovering: true, partial: true });
    state = feed(endRecovery(state, AGENT), [text(4, " world")]);
    expect(state.streams[INV]).toMatchObject({ text: " world", partial: true, recovering: false });
  });
});
