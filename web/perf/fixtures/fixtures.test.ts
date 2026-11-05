import { describe, expect, test } from "vitest";
import { DAG_SIZE, taskGraph, withState } from "./dag.ts";
import { deltaLoad, FPS, STREAMING_AGENTS, TEXT_DELTAS_PER_SECOND } from "./deltas.ts";
import { FLEET_SIZE, fleetAgents, fleetRoots } from "./fleet.ts";
import { seeded } from "./random.ts";
import { TRANSCRIPT_SIZE, transcriptEntries } from "./transcript.ts";

describe("the perf fixtures", () => {
  test("are the same for a seed and differ between seeds", () => {
    expect(Array.from({ length: 5 }, seeded(7).next)).toEqual(Array.from({ length: 5 }, seeded(7).next));
    expect(seeded(7).next()).not.toBe(seeded(8).next());
    expect(JSON.stringify(fleetAgents(1, 300))).toBe(JSON.stringify(fleetAgents(1, 300)));
    expect(JSON.stringify(fleetAgents(1, 300))).not.toBe(JSON.stringify(fleetAgents(2, 300)));
    expect(JSON.stringify(transcriptEntries(3, 500))).toBe(JSON.stringify(transcriptEntries(3, 500)));
    expect(JSON.stringify(taskGraph(4, 300))).toBe(JSON.stringify(taskGraph(4, 300)));
  });

  test("reach their stated sizes", () => {
    const agents = fleetAgents(5_000);
    expect(Object.keys(agents)).toHaveLength(FLEET_SIZE);
    const roots = fleetRoots(agents);
    expect(roots.length).toBeGreaterThan(FLEET_SIZE / 25);
    expect(Object.values(agents).every((agent) => !agent.parent || agents[agent.parent] !== undefined)).toBe(true);
    const entries = transcriptEntries(20_000);
    expect(entries).toHaveLength(TRANSCRIPT_SIZE);
    expect(entries.every((entry, i) => i === 0 || entry.seq > (entries[i - 1]?.seq ?? Infinity))).toBe(true);
    const graph = taskGraph(2_000);
    expect(Object.keys(graph.nodes)).toHaveLength(DAG_SIZE);
    expect(graph.edges.length).toBeGreaterThan(DAG_SIZE);
  });

  test("lay the DAG out acyclic, every edge from an earlier item to a later one", () => {
    const graph = taskGraph(2_000);
    for (const edge of graph.edges as unknown as { from: string; to: string }[]) {
      expect(graph.nodes[edge.from]).toBeDefined();
      expect(edge.from < edge.to).toBe(true);
    }
    const changed = withState(graph, "qi_000010", "failed");
    expect(changed.edges).toBe(graph.edges);
    expect(changed.nodes.qi_000010?.state).toBe("failed");
  });

  test("stream the delta load in contiguous orders, frame by frame", () => {
    const load = deltaLoad(60, { seconds: 2, streamers: ["a", "b", "c"], afterSeq: 100, fleetSize: 50 });
    expect(load.frames).toHaveLength(2 * FPS);
    expect(load.deltas).toBe(2 * TEXT_DELTAS_PER_SECOND * STREAMING_AGENTS);
    const last = new Map<string, number>();
    let seq = 100;
    for (const op of load.frames.flat()) {
      if (op.kind === "delta") {
        const invocation = String(op.frame.invocation);
        const order = Number(op.frame.invocation_seq);
        expect(order).toBe((last.get(invocation) ?? 0) + 1);
        last.set(invocation, order);
      }
      if (op.kind === "event") {
        expect(op.event.seq).toBeGreaterThan(seq);
        seq = op.event.seq;
      }
    }
    expect(load.events).toBe(STREAMING_AGENTS);
  });
});
