// The sustained delta rate: ten seconds of three agents streaming (one on screen, over a 20,000-entry
// transcript) and fleet rows changing, applied frame by frame through core's reducers.

import type { Store } from "@aether-ui/core";
import type { aether, ConnectionInfo } from "@aether-ui/protocol";
import { createElement, memo, type ComponentProps } from "react";
import { afterAll, describe, expect, test, vi } from "vitest";
import { applyDelta, applyEvent } from "../../packages/core/src/aether-reduce.ts";
import { Conversation } from "../../packages/aether/src/conversation/Conversation.tsx";
import { deltaLoad, FPS, type Op } from "../fixtures/deltas.ts";
import { FLEET_SIZE, fleetAgents, fleetRoots } from "../fixtures/fleet.ts";
import { TRANSCRIPT_AGENT, transcriptEntries } from "../fixtures/transcript.ts";
import { mount, perfApp, Results, timing } from "../measure.tsx";
import { renders, resetRenders } from "../renders.ts";

vi.mock("../../packages/aether/src/conversation/Message.tsx", async (importOriginal) => {
  const original = await importOriginal<typeof import("../../packages/aether/src/conversation/Message.tsx")>();
  const counter = await import("../renders.ts");
  const MessageView = memo((props: ComponentProps<typeof original.MessageView>) => {
    counter.renders.message += 1;
    return createElement(original.MessageView, props);
  });
  return { ...original, MessageView };
});

const SEED = 60;
const SECONDS = 10;
const CONN = "aether:perf";
const INFO: ConnectionInfo = { id: CONN, daemon: "aether", state: "ready", protocol: 26, features: ["deltas", "delta_order", "transcript_paging", "agents_paging"], standing: "operator", label: "perf" };
const STREAMERS = [TRANSCRIPT_AGENT, "agt_000001", "agt_000002"];
const results = new Results("deltas");
afterAll(() => results.write());

function seed(store: Store, agents: Record<string, aether.Agent>, entries: readonly aether.TranscriptEntry[]): void {
  const last = entries.at(-1)?.seq ?? 0;
  store.update((state) => ({ ...state, connections: { ...state.connections, [CONN]: INFO } }));
  store.updateAether(CONN, (state) => ({ ...state, lastSeq: last, agents, roots: fleetRoots(agents), transcripts: { [TRANSCRIPT_AGENT]: { entries, lastEntrySeq: last } } }));
}

/** One op as core's sync applies it: deltas and events through aether-reduce, rows as a get_agent reply. */
function apply(store: Store, op: Op): void {
  store.updateAether(CONN, (state) => {
    if (op.kind === "delta") {
      const { state: next, outcome } = applyDelta(state, op.frame);
      if (outcome !== "applied") throw new Error(`a load delta was ${outcome}`);
      return next;
    }
    if (op.kind === "event") return applyEvent(state, op.event);
    const held = state.agents[op.id];
    if (!held) throw new Error(`agent ${op.id} is not in the fleet`);
    return { ...state, agents: { ...state.agents, [op.id]: { ...held, status: op.status, updated_at: op.at } } };
  });
}

const percentile = (sorted: readonly number[], p: number): number => sorted[Math.min(sorted.length - 1, Math.floor(p * sorted.length))] ?? NaN;

describe("ten seconds of sustained deltas", () => {
  const agents = fleetAgents(SEED);
  agents[TRANSCRIPT_AGENT] = { ...agents.agt_000000 as aether.Agent, id: TRANSCRIPT_AGENT, name: "on-screen", parent: null };
  const entries = transcriptEntries(SEED);
  const load = deltaLoad(SEED, { seconds: SECONDS, streamers: STREAMERS, afterSeq: entries.at(-1)?.seq ?? 0, fleetSize: FLEET_SIZE });

  test("commits once a frame and redraws only the live tail", async () => {
    expect(load.frames).toHaveLength(SECONDS * FPS);
    expect(load.deltas).toBe(SECONDS * 40 * STREAMERS.length);
    const { app, frames } = perfApp();
    seed(app.store, agents, entries);
    const view = await mount(app, frames, <Conversation conn={CONN} agent={TRANSCRIPT_AGENT} />);
    await view.mutations();
    resetRenders();
    const commits = view.counters.commits;
    const frameTimes: number[] = [];
    for (const ops of load.frames) {
      const started = performance.now();
      await view.step(() => { for (const op of ops) apply(app.store, op); });
      frameTimes.push(performance.now() - started);
    }
    const state = app.store.get().aether[CONN];
    expect(state?.transcripts[TRANSCRIPT_AGENT]?.entries.length).toBeGreaterThan(entries.length);
    const sorted = [...frameTimes].sort((a, b) => a - b);
    results.count("deltas.react_commits", view.counters.commits - commits);
    results.count("deltas.row_renders", renders.message);
    results.count("deltas.dom_mutations", await view.mutations());
    results.time("deltas.frame_p50_ms", timing([percentile(sorted, 0.5)]));
    results.time("deltas.frame_p95_ms", timing([percentile(sorted, 0.95)]));
    console.log(`deltas: ${load.deltas} deltas, ${load.events} events, ${load.agentUpdates} row updates in ${load.frames.length} frames; frame p50 ${percentile(sorted, 0.5).toFixed(2)} ms, p95 ${percentile(sorted, 0.95).toFixed(2)} ms, max ${(sorted.at(-1) ?? 0).toFixed(2)} ms`);
    view.unmount();
  });
});
