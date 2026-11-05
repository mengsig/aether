// The sustained delta load: what a busy daemon sends for `seconds`, frame by frame at 60 fps. Three
// agents stream on screen (the most §5.2.2 subscribes to), each committing a message every two
// seconds, while rows of the 5,000-agent fleet change as refetched `get_agent` replies.

import type { aether } from "@aether-ui/protocol";
import { agentId, FLEET_SIZE } from "./fleet.ts";
import { seeded, stamp } from "./random.ts";

export const FPS = 60;
export const STREAMING_AGENTS = 3;
export const TEXT_DELTAS_PER_SECOND = 40;
export const AGENT_UPDATES_PER_SECOND = 20;
const COMMIT_EVERY_SECONDS = 2;

export type Op =
  | { readonly kind: "delta"; readonly frame: Readonly<Record<string, unknown>> }
  | { readonly kind: "event"; readonly event: aether.Event }
  | { readonly kind: "agent"; readonly id: string; readonly status: aether.AgentStatus; readonly at: string };

export interface DeltaLoad {
  /** `frames[i]` holds what arrives during frame i, in arrival order. */
  readonly frames: readonly (readonly Op[])[];
  readonly deltas: number;
  readonly events: number;
  readonly agentUpdates: number;
}

export interface LoadOptions {
  readonly seconds: number;
  /** The streaming agents; the first is the one on screen. */
  readonly streamers: readonly string[];
  /** Durable events get seqs after this one. */
  readonly afterSeq: number;
  readonly fleetSize?: number;
}

const STATUSES: readonly aether.AgentStatus[] = [{ state: "running" }, { state: "ready" }, { state: "completed" }, { state: "blocked", reason: { block: "inspection", detail: "waiting for an operator check" } }];

export function deltaLoad(seed: number, options: LoadOptions): DeltaLoad {
  const { seconds, streamers, afterSeq } = options;
  if (!Number.isInteger(seconds) || seconds <= 0) throw new Error(`a load lasts a whole number of seconds, not ${seconds}`);
  if (streamers.length !== STREAMING_AGENTS) throw new Error(`the load streams ${STREAMING_AGENTS} agents`);
  const random = seeded(seed);
  const fleetSize = options.fleetSize ?? FLEET_SIZE;
  const frames: Op[][] = Array.from({ length: seconds * FPS }, () => []);
  const at = (rate: number, n: number): Op[] => frames[Math.floor((n * FPS) / rate)] ?? [];
  let deltas = 0;
  let events = 0;
  streamers.forEach((agent, s) => {
    const perInvocation = TEXT_DELTAS_PER_SECOND * COMMIT_EVERY_SECONDS;
    for (let n = 0; n < seconds * TEXT_DELTAS_PER_SECOND; n += 1) {
      const invocation = `inv_live_${s}_${Math.floor(n / perInvocation)}`;
      const order = (n % perInvocation) + 1;
      at(TEXT_DELTAS_PER_SECOND, n).push({ kind: "delta", frame: { sub: s + 2, agent, invocation, invocation_seq: order, delta: { delta: "text", text: `word${n} ` } } });
      deltas += 1;
      if (order !== perInvocation) continue;
      const content: aether.ContentBlock[] = [{ type: "text", text: `Committed answer ${n}.` }];
      // Seqs are set below, in arrival order: the daemon's log orders events across agents.
      at(TEXT_DELTAS_PER_SECOND, n).push({ kind: "event", event: { seq: 0, id: "", at: "", actor: { actor: "agent", id: agent }, agent_id: agent, kind: { type: "agent.message", role: "assistant", content, invocation } } });
      events += 1;
    }
  });
  const agentUpdates = seconds * AGENT_UPDATES_PER_SECOND;
  for (let n = 0; n < agentUpdates; n += 1) {
    at(AGENT_UPDATES_PER_SECOND, n).push({ kind: "agent", id: agentId(random.below(fleetSize)), status: random.pick(STATUSES), at: stamp(100_000 + n) });
  }
  if (frames.reduce((sum, frame) => sum + frame.length, 0) !== deltas + events + agentUpdates) throw new Error("every op lands in a frame");
  return { frames: numbered(frames, afterSeq, () => 1 + random.below(3)), deltas, events, agentUpdates };
}

/** Gives the durable events ascending seqs in the order they arrive, with gaps where other events fall. */
function numbered(frames: readonly (readonly Op[])[], afterSeq: number, step: () => number): Op[][] {
  let seq = afterSeq;
  return frames.map((ops) => ops.map((op) => {
    if (op.kind !== "event") return op;
    seq += step();
    return { kind: "event", event: { ...op.event, seq, id: `evt_${seq}`, at: stamp(seq) } };
  }));
}
