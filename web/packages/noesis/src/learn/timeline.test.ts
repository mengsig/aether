// Which transcript entries Noesis wrote itself, including for a session kept before its history was.

import type { aether } from "@aether-ui/protocol";
import { describe, expect, test } from "vitest";
import { internalSeqs } from "./timeline.ts";

const entry = (seq: number, role: "user" | "assistant", text: string): aether.TranscriptEntry =>
  ({ seq, at: `2026-11-0${seq}T09:00:00Z`, message: { role, content: [{ type: "text", text }] } }) as aether.TranscriptEntry;

describe("Noesis's own inputs to an agent", () => {
  test("the seqs the daemon names are the ones held back, and nothing is guessed", () => {
    const agent: aether.SessionAgent = { agent: "agt_t", role: "teacher", internal: [4] };
    const entries = [entry(4, "user", "Prober's assessment"), entry(5, "assistant", "Hello"), entry(6, "user", "My answer")];
    expect([...internalSeqs(agent, entries)]).toEqual([4]);
  });

  test("a legacy agent with no named seqs holds back only its first input, when it came before any reply", () => {
    const legacy: aether.SessionAgent = { agent: "agt_old", role: "teacher", internal: [] };
    const opened = [entry(3, "assistant", "Hi"), entry(2, "user", "Learning goal: Raft"), entry(5, "user", "Why majorities?")];
    expect([...internalSeqs(legacy, opened)]).toEqual([2]);
    expect([...internalSeqs(legacy, [entry(1, "assistant", "Hi"), entry(2, "user", "Mine")])]).toEqual([]);
    expect([...internalSeqs({ agent: "agt_none", role: "prober" }, [])]).toEqual([]);
  });
});
