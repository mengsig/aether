// Memory 2.0 demo data: records with and without standing, and the daemon's answers to the
// memory_v2 commands. Shared by the tests and the screenshot page; nothing here is real.

import type { aether } from "@aether-ui/protocol";
import type { Script } from "../../controls/fixture/daemon.ts";

export const MEM_INVARIANT = "mem_01J0G3MEM0000000000000001A";
export const MEM_LOCKFILE = "mem_01J0G3MEM0000000000000002B";
export const MEM_CANDIDATE = "mem_01J0G3MEM0000000000000003C";
export const MEM_OLD = "mem_01J0G3MEM0000000000000004D";
export const TASK_ID = "tsk_01J0G3TASK000000000000001";
export const INVOCATION_ID = "inv_01J0G3INV0000000000000001";
export const V2_FEATURES = ["agents_paging", "transcript_paging", "deltas", "delta_order", "memory_v2"];

const base = { confidence: 1, importance: 0.8, provenance: {}, created_at: "2026-11-02T08:14:00Z" } as const;

export function demoMemories(): aether.MemoryRecord[] {
  return [
    {
      ...base, id: MEM_INVARIANT, kind: "decision", scope: { scope: "repository", id: "github.com/mengsig/aether" }, tags: ["protocol"], pinned: true,
      text: "Protocol changes ship behind a feature string; PROTOCOL_VERSION moves only for a breaking change.",
      standing: { tier: "architecture", state: "active", support: { tasks: 4, independent: 3 }, verification: "verified",
        refs: [{ kind: "path", value: "docs/memory/MEMORY-V2.md" }, { kind: "commit", value: "72a8cfc8" }] },
    },
    {
      ...base, id: MEM_LOCKFILE, kind: "lesson", scope: { scope: "global" }, tags: ["web"], created_at: "2026-10-28T16:40:00Z",
      text: "Run npm ci in web/ before the checks; the lockfile pins the toolchain.",
      standing: { tier: "operational", state: "active", support: { tasks: 2, independent: 2 }, verification: "observed",
        refs: [{ kind: "path", value: "web/package-lock.json" }],
        stale: { reason: "web/package-lock.json no longer pins vitest 4", since: "2026-11-04T11:02:00Z", revision: "c167cec8" } },
    },
    {
      ...base, id: MEM_CANDIDATE, kind: "fact", scope: { scope: "agent", id: "agt_01J0B3CONTROLS0000000001" }, tags: [], created_at: "2026-11-05T07:30:00Z",
      text: "The release lane rebuilds the desktop app on every tag.",
      standing: { tier: "working", state: "candidate", support: { tasks: 1, independent: 1 }, verification: "inferred", refs: [] },
    },
    { ...base, id: MEM_OLD, kind: "fact", scope: { scope: "global" }, tags: ["legacy"], text: "Written before admission existed; no standing is known." },
  ];
}

export function demoExplain(): aether.RetrievalExplain {
  const turn = { agent: "agt_01J0B3CONTROLS0000000001", task: TASK_ID, invocations: [INVOCATION_ID] };
  return {
    truncated: false,
    turns: [
      {
        ...turn, snapshot: "snp_01J0G3SNAP000000000000001", turn: 1, at: "2026-11-05T07:31:12Z", traced: true, pool: 9,
        injected: [MEM_INVARIANT, MEM_LOCKFILE], policy: { max_items: 6, min_score: 0.05, holdout_rate: 0.05 }, omitted: { over_limit: 0, cut: 3 },
        candidates: [
          { memory: MEM_INVARIANT, outcome: "injected", rank: 1, score: 0.92, relevance: 0.81, stale: false, pinned: true },
          { memory: MEM_LOCKFILE, outcome: "injected", rank: 2, score: 0.31, relevance: 0.62, stale: true, pinned: false },
          { memory: MEM_CANDIDATE, outcome: "held_out", rank: 3, score: 0.28, relevance: 0.44, stale: false, pinned: false },
          { memory: "mem_01J0G3MEM0000000000000005E", outcome: "trimmed", rank: 4, score: 0.2, relevance: 0.33, stale: false, pinned: false },
          { memory: "mem_01J0G3MEM0000000000000006F", outcome: "over_limit", rank: 7, score: 0.12, relevance: 0.2, stale: false, pinned: false },
          { memory: "mem_01J0G3MEM0000000000000007G", outcome: "cut", relevance: 0.01, stale: false, pinned: false },
        ],
      },
      { ...turn, snapshot: "snp_01J0G3SNAP000000000000000", turn: 0, at: "2026-11-05T07:30:02Z", traced: false, pool: 0 },
    ],
  };
}

export function demoConsolidation(dryRun: boolean): aether.MemoryConsolidation {
  return {
    dry_run: dryRun, scanned: 42,
    clusters: [
      { scope: { scope: "global" }, survivor: MEM_LOCKFILE, members: [MEM_LOCKFILE, "mem_01J0G3MEM0000000000000008H", "mem_01J0G3MEM0000000000000009J"], outcome: dryRun ? "planned" : "merged", promoted: !dryRun },
      { scope: { scope: "repository", id: "github.com/mengsig/aether" }, survivor: MEM_INVARIANT, members: [MEM_INVARIANT, "mem_01J0G3MEM000000000000000AK"], outcome: dryRun ? "planned" : "skipped", reason: dryRun ? null : "a member changed under the plan" },
    ],
  };
}

export const DEMO_ARTIFACT = [
  '{"format":"aether.memory-export","version":1,"exported_at":"2026-11-05T06:00:00Z","schema":32,"scope":null,"records":2}',
  `{"record":{"id":"${MEM_INVARIANT}"},"admission":null,"stale":null}`,
  `{"record":{"id":"${MEM_OLD}"},"admission":null,"stale":null}`,
].join("\n");

export function demoImport(dryRun: boolean): aether.MemoryImport {
  return {
    dry_run: dryRun, digest: "9f2c41d07be3a8e6c5b1f0e2d4a6c8b0e1f3a5c7d9e2b4f6a8c0e1d3f5a7b9c1", records: 12, inserted: 9, unchanged: 2,
    conflicts: [{ memory: MEM_OLD, reason: "the stored text differs from the exported one" }],
  };
}

export function demoMigrationStatus(): aether.MemoryMigrationStatus {
  return {
    schema_version: 32,
    migrations: [{ version: 30, name: "memory_admission", applied: true }, { version: 31, name: "memory_stale", applied: true }, { version: 32, name: "memory_v2_marker", applied: true }],
    memories: { total: 128, admitted: 117, implicit: 11, candidates: 6, stale: 3, held_out: 14 },
    backups: [
      { name: "schema-31-before-32-01J0G3.sqlite.zst", from_version: 31, to_version: 32, bytes: 4_812_306, at: "2026-11-05T05:58:41Z" },
      { name: "schema-29-before-30-01J0F1.sqlite.zst", from_version: 29, to_version: 30, bytes: 4_120_011, at: "2026-10-30T09:12:03Z" },
    ],
    last_import: { at: "2026-11-05T06:10:00Z", digest: "9f2c41d07be3a8e6c5b1f0e2d4a6c8b0e1f3a5c7d9e2b4f6a8c0e1d3f5a7b9c1", inserted: 9, unchanged: 2, conflicts: 1 },
  };
}

/** Every memory read and memory_v2 command the screen sends, answered from the demo data. */
export function memoryScript(): Script {
  return {
    memories: (command) => ({ tag: "memories", data: (command.query as { proposed?: boolean }).proposed ? [] : demoMemories() }),
    memory_retrieval_explain: () => ({ tag: "memory_retrieval_explain", data: demoExplain() }),
    consolidate_memories: (command) => ({ tag: "memory_consolidation", data: demoConsolidation(command.dry_run === true) }),
    memory_export: () => ({ tag: "memory_export", data: { artifact: DEMO_ARTIFACT, records: 2, digest: "5d41402abc4b2a76b9719d911017c592ae2f5b0c3a7e0f1d2c4b6a8e9f0d1c2b" } }),
    memory_import: (command) => ({ tag: "memory_import", data: demoImport(command.dry_run === true) }),
    memory_migration_status: () => ({ tag: "memory_migration_status", data: demoMigrationStatus() }),
  };
}
