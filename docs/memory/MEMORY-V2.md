# Memory 2.0 on the wire (settles CONTRACTS.md §7)

This is the settled text of CONTRACTS.md §7 for the Aether side of Memory
2.0 (G1–G4). G3 exposes on the wire what G1, G2 and G4 built. It does not
change admission, retrieval or consolidation. Archon lessons (G5) are not
covered here.

Everything below is in Aether protocol 26 behind the feature string
`memory_v2`. `PROTOCOL_VERSION` does not change. A client that does not
list `memory_v2` in its hello sees every reply exactly as before.

## 1. Record standing

`MemoryRecord` gains one optional field, `standing`. It is filled in on the
`memories` and `memory` replies, and only for a client whose hello lists
`memory_v2`. It is never stored in the record body. The store keeps it in
`memory_admission` (V30) and `memory_stale` (V31).

```text
MemoryRecord.standing?: {
  tier: "working" | "operational" | "architecture" | "history" | "evidence",
  state: "candidate" | "active" | "superseded" | "retracted",
  support: { tasks: u32, independent: u32 },
  verification: "inferred" | "observed" | "verified",
  refs: [{ kind: "repo"|"path"|"symbol"|"commit"|"issue"|"pr"|"test"|"review"|"doc", value: string }],
  stale?: { reason: string, since: Ts, revision?: string }
}
```

- `state` is `MemoryState::of(record, stored)`. Superseded and retracted
  come from the record (`superseded_by`, `forgotten_at`). A pending operator
  proposal is a candidate. A memory with no admission row (one written
  before V30's backfill could reach it) is served as G1 derives it: active,
  with no support.
- `support.tasks` counts the distinct supporting tasks, and
  `support.independent` counts their distinct runs. Promotion needs 2
  independent runs (G1).
- `stale` is present when retrieval demotes the memory (G2: half score,
  never hidden). `revision` is the commit whose tree stopped stating the
  memory, when a repository sweep set the mark.
- `verification: "inferred"` is an agent's claim and is never shown as
  fact (ARCHITECTURE §h.6).

## 2. Commands

| Command | Shape | Reply | Class | Remote |
| --- | --- | --- | --- | --- |
| `memory_retrieval_explain` | `{task?: TaskId, invocation?: InvocationId}`, exactly one | `memory_retrieval_explain` | host_only | refused to every paired device |
| `consolidate_memories` | `{scope?: MemoryScope, dry_run: bool}` | `memory_consolidation` | host_only | refused to every paired device |
| `memory_export` | `{scope?: MemoryScope}` | `memory_export` | host_only | refused to every paired device |
| `memory_import` | `{artifact: string, dry_run: bool}` | `memory_import` | host_only | refused to every paired device |
| `memory_migration_status` | `{}` | `memory_migration_status` | observe | allowed, observers included |

Each command is listed in `Command::introduced()` as
`Feature("memory_v2")`, classified in `access.rs`, listed in
`docs/remote.md`, and checked against `authz/command_cases.json`.

`dry_run` has no default on either command, so a client must say which it
means. `scope` is the wire `MemoryScope` (`{"scope":"global"}`,
`{"scope":"repository","id":…}`, and so on). It is matched exactly, with the
role scopes nested in a repository. An absent scope means every scope.

### 2.1 `memory_retrieval_explain`

This command reports what retrieval did for each model call of a task, or
for the one call of an invocation. The trace is recorded when the turn is
built, in the snapshot it explains (`InvocationSnapshot.memory_retrieval`),
so it reports what happened then, not what retrieval would do now.

```text
reply memory_retrieval_explain {
  turns: [{
    snapshot: SnapshotId, agent: AgentId, task?: TaskId, turn: u32, at: Ts,
    invocations: [InvocationId],
    traced: bool,                  // false: the snapshot predates G3, or the turn did not retrieve
    injected: [MemoryId],          // what the model was given: the snapshot's memory layer, in render order
    policy?: { max_items: u32, min_score?: f32, holdout_rate: f32 },
    pool: u32,                     // candidates considered
    candidates: [{
      memory: MemoryId,
      outcome: "injected" | "held_out" | "trimmed" | "over_limit" | "cut",
      rank?: u32,                  // 1-based rank among the memories that passed the cutoff
      score?: f32,                 // final score; absent for a cut candidate, which is never scored
      relevance: f32,              // what the cutoff compared with min_score
      stale: bool,                 // demoted at half score
      pinned: bool
    }],
    omitted: { over_limit: u32, cut: u32 }   // candidates past the trace caps
  }],
  truncated: bool                  // a task with more than 256 snapshots
}
```

Outcomes:

- `cut`: the relevance was below `min_score`, so the memory was dropped
  before ranking. Pinned memories are never cut. With no query, nothing is
  cut and `min_score` is absent.
- `over_limit`: the memory passed the cutoff but ranked past `max_items`.
- `trimmed`: the memory was within `max_items`, but the model's memory
  budget had no room for it.
- `held_out`: the memory fit the budget, but the holdout drew it (logged in
  `memory_holdouts`).
- `injected`: the memory is in the snapshot's memory layer.

`injected` is read from the snapshot itself, so the reply always says what
the model was given. A trace entry marked for injection that is not in the
layer is reported as `trimmed`. The trace keeps every memory within
`max_items`, the 16 best over-limit memories and the 16 most relevant cut
memories, and counts the rest in `omitted`.

Errors: `invalid_argument` when neither or both of task and invocation are named;
`not_found` for an unknown task or invocation, or a snapshot that retention
removed.

### 2.2 `consolidate_memories`

This command runs G4's `Runtime::consolidate_memories` with abstraction off.

```text
reply memory_consolidation {
  dry_run: bool, scanned: u32,
  clusters: [{ scope: MemoryScope, survivor: MemoryId, members: [MemoryId],
               outcome: "planned" | "merged" | "skipped", promoted: bool, reason?: string }]
}
```

`planned` means a dry run: nothing was written. `merged` means the members
are now superseded by the survivor, and `promoted` says whether the merged
support made a candidate survivor active. `skipped` carries the reason
(something changed under the plan).

### 2.3 `memory_export` and `memory_import`

```text
reply memory_export { artifact: string, records: u32, digest: string }   // digest: SHA-256 hex of artifact
reply memory_import { dry_run: bool, digest: string, records: u32, inserted: u32, unchanged: u32,
                      conflicts: [{ memory: MemoryId, reason: string }] }
```

Export reads every memory in the scope, including candidates, pending
proposals, superseded memories and forgotten ones. It reads them under
the commit lock, so the artifact is one snapshot of the store. More than
100,000 records, or an artifact over 48 MiB as an escaped JSON string, is
refused as `invalid_argument`, with the advice to export per scope. The byte bound
keeps the reply, and the import that sends it back, inside a 64 MiB frame.

Import compares each entry with the store by memory id:

- An entry the store lacks is inserted exactly as exported, with its
  admission row and stale mark.
- An entry identical to the stored one is `unchanged`.
- An entry that differs from the stored one is a conflict. It is reported
  and never applied: an import never overwrites a local memory.

All inserts go in one commit, with one `memory.imported` event, so an
import lands whole or not at all. Importing the same artifact again inserts
nothing. `dry_run` reports the same counts and writes nothing.

Import errors (`invalid_argument`, in a dry run too, and nothing is written):
- a missing or unknown header;
- a version newer than this build reads;
- a record count that does not match the header;
- a malformed entry, naming its line;
- a repeated memory id;
- an admission naming another memory;
- an admission state other than candidate or active;
- a proposal state the store refuses;
- an entry carrying `standing`;
- an `importance` or `confidence` that is not a finite number in 0..=1
  (a JSON number past f32 would parse as infinity);
- a stale note with a blank reason or a blank revision.

The web import refuses, before sending, a file whose escaped JSON string
is over the same 48 MiB bound, so it takes every artifact an export gives.

### 2.4 Export artifact format

The artifact is UTF-8 JSON Lines, versioned and self-describing.

Line 1 is the header:

```json
{"format":"aether.memory-export","version":1,"exported_at":"2026-11-05T06:00:00Z","schema":32,"scope":null,"records":2}
```

Each following line is one memory, ordered by `created_at` and then by id:

```json
{"record":{…MemoryRecord, without standing…},"admission":{…MemoryAdmission…}|null,"stale":{"reason":"…","since":"…","revision"?:"…"}|null}
```

- `record` is the stored body, every field included: ids, scope,
  provenance, supersession links, proposal, pin, forget and retraction
  state, and retrieval counts.
- `admission` is the stored V30 row: tier, state, the supporting tasks with
  their runs, verification, refs and `promoted_at`. It is `null` when the
  store holds none, and an import then writes none, so the round trip stays
  exact.
- `stale` is the V31 mark, or `null`. Its `revision` is absent when no
  sweep set the mark.

A reader refuses a version it does not know. A later version may add
optional fields. Feedback, the holdout log and retrieval traces are not
exported, because they name tasks and runs of the store that wrote them.

### 2.5 `memory_migration_status`

```text
reply memory_migration_status {
  schema_version: u32,                       // the store's stamp
  migrations: [{ version: u32, name: string, applied: bool }],   // V30, V31, V32
  memories: { total: u32, admitted: u32, implicit: u32, candidates: u32, stale: u32, held_out: u32 },
  backups: [{ name: string, from_version: u32, to_version: u32, bytes: u64, at: Ts }],
  last_import?: { at: Ts, digest: string, inserted: u32, unchanged: u32, conflicts: u32 }
}
```

- `implicit` counts memories with no admission row, which are read as
  active.
- `held_out` counts rows in the holdout log.
- `last_import` is the last applied import. It is kept in the store's meta
  key `memory.last_import` as well as the `memory.imported` event, so the
  status reads it in one lookup.
- `backups` lists the pre-migration snapshots in `migration-backups/`
  beside the store, newest first, by file name only: no host path is sent
  to a device.

## 3. Store migration V32

V32 ships empty, so the numbering stays fixed (CONTRACTS §1.4):
- export and import need no bookkeeping table, because idempotency comes
  from comparing ids and bodies;
- the import record is the `memory.imported` event;
- the retrieval trace lives in the snapshot body.

The version stamp still moves from 31 to 32. Opening a V31 store therefore
takes the pre-migration backup `schema-31-before-32-<id>.sqlite.zst`, as
every migration does (docs/migration-backups.md).

**Rollback:** stop the daemon, restore that backup as
docs/migration-backups.md describes, and start the previous binary. The
backup is a V31 store with every row intact. The previous binary refuses
the V32 store itself as `schema too new`. The upgrade lane's `REWINDS`
entry for 32 is empty: only the stamp goes back.

## 4. Config

The keys are as G2 shipped them, under `[memory.retrieval]`:

| Key | Default | Range |
| --- | --- | --- |
| `max_items` | 6 | 1–50 |
| `min_score` | 0 (off; G2 shipped 0.05) | 0–1 |
| `holdout_rate` | 0.05 | 0–1 |
| `holdout_seed` | 0 | u64 |

`admission.mode` is not a config key yet. G1 keeps `Immediate` as the
library default, and turning candidates on is a later behaviour change with
its own benchmark.

## 5. CLI

| Command | Calls |
| --- | --- |
| `aether memory explain (--task T \| --invocation I) [--json]` | `memory_retrieval_explain` |
| `aether memory consolidate [--scope S] [--dry-run] [--json]` | `consolidate_memories` |
| `aether memory export [--scope S] [-o FILE] [--json]` | `memory_export`; writes the artifact to FILE, or to stdout |
| `aether memory import FILE [--dry-run] [--json]` | `memory_import` |
| `aether memory migration-status [--json]` | `memory_migration_status` |

`--scope` takes what `aether memory list --scope` takes: `global`,
`role:<role>`, `agent:<agent>`, `subtree:<agent>`,
`repo-role:<role>@<repository>`, or anything else as a repository.

`export -o FILE` writes a staging file it creates under a fresh name
beside FILE, then renames it onto FILE, so FILE is never half an artifact. With `--json` it prints the count and digest.

## 6. Changes from the §7 proposal

- The six fields are grouped under one optional `standing` object instead
  of sitting flat on `MemoryRecord`. They are present together or not at
  all, they are never written into the stored record body, and a record
  stays one serde struct without `flatten`, which would slow every store
  read.
- `stale` gains an optional `revision`.
- `consolidate_memories.scope` is optional (absent means every scope), and
  `dry_run` is required on both writing commands.
- `memory_export` returns the artifact inline as JSONL text with its
  SHA-256, and `memory_import` takes that text. No artifact id or host path
  is involved, so a browser can download and upload it.
- Import never overwrites: a differing memory is a reported conflict.
- `memory_migration_status`'s reply shape is new (the proposal named the
  command only).
- `retrieval.max_items`, `min_score` and `holdout_rate` live under
  `[memory.retrieval]`, as G2 shipped them. `admission.mode` is deferred.
- V32 is a no-op.
