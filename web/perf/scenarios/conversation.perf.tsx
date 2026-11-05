// An agent's conversation at 20,000 transcript entries: mount, scroll, a streamed delta and a
// committed message, through core's own reducers (aether-reduce.ts).

import type { Store } from "@aether-ui/core";
import type { aether, ConnectionInfo } from "@aether-ui/protocol";
import { createElement, memo, type ComponentProps } from "react";
import { afterAll, describe, expect, test, vi } from "vitest";
import { applyDelta, applyEvent } from "../../packages/core/src/aether-reduce.ts";
import { Conversation } from "../../packages/aether/src/conversation/Conversation.tsx";
import { entryRows, pairTools } from "../../packages/aether/src/conversation/rows.ts";
import { stamp } from "../fixtures/random.ts";
import { TRANSCRIPT_AGENT, TRANSCRIPT_SIZE, transcriptEntries } from "../fixtures/transcript.ts";
import { heapUsed, mount, perfApp, Results, timed, type Mounted } from "../measure.tsx";
import { renders, resetRenders } from "../renders.ts";

// Every conversation row draws one MessageView, so its renders count row renders.
vi.mock("../../packages/aether/src/conversation/Message.tsx", async (importOriginal) => {
  const original = await importOriginal<typeof import("../../packages/aether/src/conversation/Message.tsx")>();
  const counter = await import("../renders.ts");
  const MessageView = memo((props: ComponentProps<typeof original.MessageView>) => {
    counter.renders.message += 1;
    return createElement(original.MessageView, props);
  });
  return { ...original, MessageView };
});

const SEED = 20_000;
const CONN = "aether:perf";
const INFO: ConnectionInfo = { id: CONN, daemon: "aether", state: "ready", protocol: 26, features: ["deltas", "delta_order", "transcript_paging"], standing: "operator", label: "perf" };
const results = new Results("conversation");
afterAll(() => results.write());

const AGENT: aether.Agent = {
  id: TRANSCRIPT_AGENT, name: "builder", role: "worker", run: "run_perf", model: { provider: "claude-code", model: "opus" },
  status: { state: "running" }, config: {}, scope: {}, created_at: stamp(0), updated_at: stamp(0),
};

function seed(store: Store, entries: readonly aether.TranscriptEntry[]): void {
  const last = entries.at(-1)?.seq ?? 0;
  store.update((state) => ({ ...state, connections: { ...state.connections, [CONN]: INFO } }));
  store.updateAether(CONN, (state) => ({
    ...state, lastSeq: last, agents: { [AGENT.id]: AGENT }, roots: [AGENT.id], transcripts: { [AGENT.id]: { entries, lastEntrySeq: last } },
  }));
}

async function mountConversation(entries: readonly aether.TranscriptEntry[]): Promise<Mounted> {
  const { app, frames } = perfApp();
  seed(app.store, entries);
  return mount(app, frames, <Conversation conn={CONN} agent={AGENT.id} />);
}

function textDelta(store: Store, order: number): void {
  store.updateAether(CONN, (state) => {
    const frame = { agent: AGENT.id, invocation: "inv_live", invocation_seq: order, delta: { delta: "text", text: `word${order} ` } };
    const { state: next, outcome } = applyDelta(state, frame);
    if (outcome !== "applied") throw new Error(`delta ${order} was ${outcome}`);
    return next;
  });
}

function commitMessage(store: Store, n: number): void {
  store.updateAether(CONN, (state) => {
    const seq = (state.lastSeq ?? 0) + 1;
    const event = {
      seq, id: `evt_${seq}`, at: stamp(seq), actor: { actor: "agent", id: AGENT.id }, agent_id: AGENT.id,
      kind: { type: "agent.message", role: "assistant", content: [{ type: "text", text: `Committed answer ${n}.` }], invocation: `inv_commit_${n}` },
    } as aether.Event;
    return applyEvent(state, event);
  });
}

describe("the conversation at 20,000 entries", () => {
  const entries = transcriptEntries(SEED);

  test("mounts a screenful of messages", async () => {
    expect(entries).toHaveLength(TRANSCRIPT_SIZE);
    const mountTime = await timed(async () => { (await mountConversation(entries)).unmount(); });
    resetRenders();
    const before = heapUsed();
    const conv = await mountConversation(entries);
    const retained = heapUsed() - before;
    // A tool result shows inside its call, so the list has fewer rows than entries.
    const rows = entryRows(entries, pairTools(entries), new Map()).length;
    expect(rows).toBeGreaterThan(TRANSCRIPT_SIZE * 0.7);
    expect(conv.container.querySelector("[role=listitem]")?.getAttribute("aria-setsize")).toBe(String(rows));
    results.time("conversation.mount.ms", mountTime);
    results.count("conversation.mount.commits", conv.counters.commits);
    results.count("conversation.mount.dom_elements", conv.container.querySelectorAll("*").length);
    results.count("conversation.mount.row_renders", renders.message);
    results.amount("conversation.mount.heap_bytes", retained);
    conv.unmount();
  });

  test("scrolls back a screen at a time", async () => {
    const conv = await mountConversation(entries);
    const list = conv.container.querySelector<HTMLElement>(".conv-list");
    if (!list) throw new Error("the conversation has no list");
    let page = 0;
    const scroll = () => conv.step(() => {
      page += 1;
      list.scrollTop = page * 600;
      list.dispatchEvent(new Event("scroll"));
    });
    await conv.mutations();
    resetRenders();
    await scroll();
    expect(conv.container.querySelector("[role=listitem]")?.getAttribute("aria-posinset")).not.toBe("1");
    results.count("conversation.scroll.row_renders", renders.message);
    results.count("conversation.scroll.dom_mutations", await conv.mutations());
    results.time("conversation.scroll.ms", await timed(scroll));
    conv.unmount();
  });

  test("applies one streamed text delta", async () => {
    const conv = await mountConversation(entries);
    const store = conv.app.store;
    let order = 0;
    const delta = () => conv.step(() => { order += 1; textDelta(store, order); });
    await delta();
    await conv.mutations();
    resetRenders();
    const commits = conv.counters.commits;
    await delta();
    expect(store.get().aether[CONN]?.streams.inv_live?.text).toContain("word2");
    results.count("conversation.delta.commits", conv.counters.commits - commits);
    results.count("conversation.delta.row_renders", renders.message);
    results.count("conversation.delta.dom_mutations", await conv.mutations());
    results.time("conversation.delta.ms", await timed(delta));
    conv.unmount();
  });

  test("appends one committed message", async () => {
    const conv = await mountConversation(entries);
    const store = conv.app.store;
    let n = 0;
    const commit = () => conv.step(() => { n += 1; commitMessage(store, n); });
    await conv.mutations();
    resetRenders();
    await commit();
    expect(store.get().aether[CONN]?.transcripts[AGENT.id]?.entries).toHaveLength(TRANSCRIPT_SIZE + 1);
    results.count("conversation.commit.row_renders", renders.message);
    results.time("conversation.commit.ms", await timed(commit));
    conv.unmount();
  });
});
