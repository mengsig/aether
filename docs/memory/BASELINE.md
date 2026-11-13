# Operational memory: baseline

This page describes Aether's operational memory as it stands before Memory 2.0,
and records its baseline on a reproducible benchmark. Memory 2.0 changes are
judged against these numbers, on the same dataset and under the same
conditions.

- **System under test:** the memory code of `ad7d94b8` (Aether 1.2.11, `next`).
  The benchmark adds a driver and makes one function public
  (`aether_context::memory_layer`); it changes no behaviour.
- **Baseline recorded at:** `b0ffd05e42fa8fff3bb2fd4b548f893edbfb8b8d`, the
  commit that adds the benchmark, on a clean tree.
- **Reproduce:** `make bench-memory`. It builds the driver, runs both arms in
  private sandboxes, prints the report and exits 1 unless every deterministic
  metric equals [`bench/memory-set/v1/baseline.json`](../../bench/memory-set/v1/baseline.json).
  It calls no model. That file now holds the re-recording at 1.3.0 described
  in [Re-recorded at 1.3.0](#re-recorded-at-130); the tables below are the
  original 1.2.11 baseline.

## How memory works today

### Proposal and admission

1. An agent calls the `remember` tool
   (`crates/aether-tools/src/builtin/harness/memory.rs`) with text, kind,
   importance, confidence, tags, a scope and optionally the id it supersedes.
2. The runtime (`crates/aether-runtime/src/turn/actions/memory.rs`) stamps the
   agent and task into the provenance, resolves the `role` scope to the
   agent's role, and asks `MemoryService::evaluate`
   (`crates/aether-memory/src/admission.rs`) under the write gate.
3. Admission (`AdmissionPolicy` in `crates/aether-memory/src/lib.rs`) rejects a
   candidate that is shorter than 12 or longer than 1,200 characters, has
   importance below 0.3 or confidence below 0.4, or contains a transient
   marker ("currently", "for now", "temporarily", "while debugging", "todo:",
   ...).
4. A duplicate is only the **same trimmed text** in the same scope
   (`find_duplicate`). It is rejected as redundant unless the new candidate
   has higher importance or confidence, in which case it supersedes the old
   one. A reworded restatement is never detected.
5. An explicit `supersedes` marks the old record superseded, if it is current,
   in the same scope, and not authored by an operator, pinned, or read from a
   repository file (`supersession_refused`). Nothing else invalidates a fact:
   a newer memory that contradicts an older one leaves both current.
6. Every step is an event (`MemoryCandidate`, `MemoryAccepted`,
   `MemoryRejected`, `MemorySuperseded`); records are never deleted.

Operators add memories directly or through review
(`crates/aether-runtime/src/memory_curation.rs`,
`crates/aether-memory/src/proposals.rs`), and can forget, restore and pin
them (`MemoryService::forget`, `restore`, `pin`). Repository notes committed
under the tree are seeded as memories and retracted when they leave it
(`crates/aether-memory/src/seeding.rs`, `repo.rs`).

### Storage and scope

Records live in the SQLite store's `memories` table with an FTS5 index
(`crates/aether-store/src/sqlite/memories.rs`). A scope is one of `global`,
`repository`, `role`, `agent`, `subtree` or `repository_role`
(`crates/aether-core/src/memory.rs`); repository scopes resolve through
registered aliases (`MemoryService::canonical_scope`).

### Retrieval and ranking

`MemoryService::retrieve` (`crates/aether-memory/src/retrieval.rs`):

1. **Scopes:** the agent's repository, that repository's role families, the
   role, the agent and up to eight ancestors' subtrees; global memories are in
   every page.
2. **Candidate pool:** per scope, the `4 x limit` most important memories and,
   with a query, the `4 x limit` best BM25 matches of an OR of its words.
3. **Score:** `1.0 if pinned + 0.3 importance + 0.1 confidence + scope
   (0.15 global .. 0.35 repository role) + 0.1 / (1 + age_days / 30) + 0.15 tag
   overlap + 0.2 query-word overlap + 0.25 semantic similarity`. The semantic
   pass runs only with `[memory] index = "local"` (a trigram hash embedder,
   `crates/aether-memory/src/index.rs`); the default is `none`.
4. **Feedback:** the score of a non-pinned memory with at least two counted
   verdicts is multiplied by 0.75 to 1.25 (`FeedbackTally::factor`,
   `crates/aether-core/src/memory_feedback.rs`).
5. The top `limit` are returned. There is no relevance threshold: a turn
   always gets `limit` memories when that many are in scope.

### Injection

`retrieve_memories` (`crates/aether-runtime/src/turn/context.rs`) ranks on the
task objective plus the new inbox messages, with `limit` 6, on every API turn
(`turn/execution.rs`); engine sessions rank on the objective only and resend
the layer only when it changes (`turn_engine/execution.rs`). The context
assembler renders each memory as one line with its id, tags, origin and date
(`render::memory_line`) under a cap of `window / 40` tokens, clamped to 400 to
2,000 (`crates/aether-context/src/budget.rs`); repository notes go in a
separate block marked untrusted. Each snapshot records the ids it showed
(`aether memory context`), and `touch` counts retrievals.

### Outcome feedback

`Runtime::record_memory_feedback` (`crates/aether-runtime/src/memory_feedback.rs`)
appends an operator's `helped`, `hurt` or `neutral` verdict with `operator`,
`review` or `outcome` evidence; only `operator` and `review` verdicts move
ranking (`RANKED_EVIDENCE`), over the 20 most recent tasks. Feedback can be
retracted. Nothing records whether a retrieved memory changed what an agent
did.

## The benchmark

`evals/memory_bench.py` writes a plan from the dataset and runs the driver,
`crates/aether-memory/examples/memory_bench/`, against a fresh SQLite store in
a sandbox under `target/memory-bench/`, removed afterwards. The driver uses the
paths agents use: `MemoryService::admit` with the agent or operator actor,
`retrieve` with the request a turn builds, and `aether_context::memory_layer`
for what the model is sent. Three fixture writes go to the store directly,
outside those paths:

- **Backdating:** each record's `created_at` is set back by its `age_days`,
  because recency is a ranking signal and no public path records an old
  memory.
- **Feedback:** review verdicts are committed as `RecordMemoryFeedback`, each
  with an invented task id, bypassing `Runtime::record_memory_feedback` and
  its task and context checks. The benchmark measures how seeded feedback
  ranks, not how feedback is accepted.
- **Filler:** the 5,000 memories of the scaled latency store are written as
  records, without admission or its events, so the scaled bytes per memory
  understate an admitted store's.

### Dataset

`bench/memory-set/v1/` holds six synthetic scenarios (54 candidate memories,
26 labelled queries): a Rust service's CI, a Python API's migrations, a Go
CLI's releases, a TypeScript dashboard, cross-repository operational lessons
in global and role scope, and a repository with no memories of its own. Every
repository, path, commit and number is invented. Each memory has a class:

| Class | Count | Meaning |
|---|---|---|
| `durable` | 26 | A reusable invariant, procedure or lesson |
| `near_duplicate` | 7 | Restates a durable memory in other words (`duplicate_of`) |
| `one_off` | 11 | A hyper-specific observation that belongs in task history |
| `stale` | 5 | Outdated; a newer memory `contradicts` it without superseding it |
| `incorrect` | 1 | A wrong lesson that review marked as hurting twice |
| `transient` | 4 | Should never be admitted |

Two durable memories are replaced through `supersedes`, all five stale ones
are contradicted by newer memories, and two memories carry review feedback
(one durable that helped, the incorrect one that hurt). Each query labels the
memories it needs (`relevant`, always a cluster's first wording) and the ones
that would mislead it (`harmful`). Three queries have no relevant memory. Seven carry a question and an answer pattern for the task
arm.

### Metrics

Deterministic, checked against the baseline:

- **Retrieval:** precision@k is the share of the k ranked slots that hold a
  relevant memory not already shown in another wording (with no slot ranked,
  1 when nothing is relevant); recall@k is the share of relevant memories
  shown; MRR over queries with a relevant memory. k is 3 and 6.
- **Influence:** counted on the memories injected into the layer, not on the
  ranking: queries with a harmful label that are shown one; share of all
  injected slots holding one; and for each stale or superseded fact whose
  replacement the query needs, how often the old one is shown first or alone.
- **Store:** shares of the current memories the store lists after seeding
  that restate another, are one-off, or are stale or incorrect; the admission
  rate of near-duplicates and the rejection rate of transients.
- **Injection:** memories and estimated tokens injected per turn; the share of
  estimated memory-line tokens (the layer header excluded) spent on lines
  that are not relevant; tokens injected when nothing is relevant; injected
  slots repeating a shown memory or holding a one-off; model calls per
  retrieval.
- **Recovery:** whether every query ranks the same after closing and reopening
  the store; and, on copies of the closed database, what opening and
  retrieving do after each kind of damage.

Measured, reported but not checked: retrieval latency over 30 repetitions per
query on the dataset store and on a scaled store with 5,000 synthetic
memories (inserted directly, see above), and the database's bytes per record.

The task arm (`--task-arm`, off by default, capped by `--task-max-calls`)
asks a model each task's question with no memory, with the layer retrieval
produced, and with only the relevant memories. Because the facts are invented,
the paired difference on the same tasks is evidence that the memory caused the
answer; success split by whether retrieval surfaced a relevant memory is
reported separately, as correlation only.

## Baseline

Recorded at `b0ffd05e42fa8fff3bb2fd4b548f893edbfb8b8d` with `make bench-memory`
(both arms; `index=none` is the shipped default). Latency and bytes come from a
12-core Linux host and vary by machine.

| Metric | index=none | index=local |
|---|---|---|
| Precision@3 | 0.2564 | 0.2949 |
| Precision@6 | 0.1603 | 0.1731 |
| Recall@3 | 0.6957 | 0.8261 |
| Recall@6 | 0.8913 | 0.9783 |
| MRR | 0.7297 | 0.8333 |
| Queries shown a stale or incorrect memory | 0.4 | 0.6 |
| Slots holding a stale or incorrect memory | 0.0256 | 0.0385 |
| Conflicts where the old fact ranks first or alone | 0.125 | 0.5 |
| Stored memories that restate another | 0.1458 | 0.1458 |
| Stored memories that are one-off observations | 0.2292 | 0.2292 |
| Stored current memories that are stale or incorrect | 0.125 | 0.125 |
| Near-duplicates admitted | 1.0 | 1.0 |
| Transient candidates rejected | 1.0 | 1.0 |
| Memories injected per turn | 6 | 6 |
| Injected tokens per turn, mean / p95 | 323 / 349 | 320 / 346 |
| Memory-line tokens not relevant to the task (header excluded) | 0.8301 | 0.815 |
| Tokens injected when nothing is relevant | 305 | 307 |
| Slots repeating a memory already shown | 0.2244 | 0.1731 |
| Slots holding a one-off observation | 0.1795 | 0.2115 |
| Model calls per retrieval | 0 | 0 |
| Queries ranked identically after restart | 1.0 | 1.0 |
| Retrieval latency, 48 memories, median / p95 | 2.17 / 3.13 ms | 2.29 / 3.20 ms |
| Retrieval latency, 5,048 memories, median / p95 | 8.23 / 12.28 ms | 9.16 / 12.26 ms |
| Database bytes per record (events included) | 3,031 | 3,031 |
| Database bytes per memory, scaled store | 1,310 | 1,312 |

Recovery, the same for both arms:

| Damage to the closed database | Open | Retrieval | Memories recovered |
|---|---|---|---|
| None (intact copy, the control) | ok | ok | 48 of 48 |
| File missing | ok | ok | 0 of 48: a new, empty store, silently |
| Truncated to half | error: database disk image is malformed | - | - |
| First 100 bytes zeroed | error: file is not a database | - | - |
| One middle page overwritten (the page at half the file) | ok | ok | 48 of 48: damage not detected |
| Every eighth page overwritten | error: database disk image is malformed | - | - |

The WAL is not truncated on close: the dataset store occupies 5.7 MB on disk
for a 0.6 MB database.

Task arm, run once with `python3 evals/memory_bench.py run --index none
--task-arm --task-max-calls 21` (Haiku through `claude -p --model haiku --tools ""`,
21 calls over 7 tasks): success 0 of 7 with no memory, 7 of 7 with the retrieved
layer and 7 of 7 with the relevant memories alone. The paired effect of
retrieval is +1.0; the set is at the ceiling for it, so the arm cannot yet
separate a better ranking from the current one. In the two tasks whose layer
also held the stale fact, the model still answered with the newer one.

### What the baseline shows

- **Inject sparingly is not met.** Every turn gets six memories, about 320
  tokens, whatever the task; 83% of the memory-line tokens are not relevant,
  and a task with nothing relevant still gets about 305 tokens of memory.
- **Duplicates and one-offs are stored and shown.** Every reworded
  restatement is admitted (15% of the store), so a fifth of retrieved slots
  repeat a memory already shown. Every one-off observation is admitted (23%
  of the store) and fills 18% of slots.
- **Contradictions persist.** A newer fact that does not name the old one
  leaves both current; 40% of queries with a harmful label are shown the stale
  or incorrect memory. Explicit supersession works: no superseded memory was
  ever retrieved.
- **Semantic similarity alone is not better.** The local index raises recall
  (0.89 to 0.98) but also raises stale exposure (0.4 to 0.6) and the rate at
  which the old fact ranks ahead of its replacement (0.125 to 0.5), because
  an outdated statement is worded like its replacement.
- **Recovery is all or nothing in the cases tested.** A truncated database,
  a zeroed header and damage to every eighth page refuse to open; one
  overwritten middle page goes undetected and every memory is still
  retrieved; a missing file is replaced by an empty store without a warning.
  No path restores from a backup.

### Re-recorded at 1.3.0

`bench/memory-set/v1/baseline.json` was recorded again at
`111d31c96d3441f1df4fea54b4115c892af1313b` (`next` after 1.3.0) with
`python3 evals/memory_bench.py run --write-baseline --load-max 1000`, on a
clean tree. Every deterministic metric above is unchanged except one
recovery case:

- **One page overwritten** now damages the middle leaf page of the `memories`
  table, found by walking the b-tree page headers from the table's root, and
  no longer the page at half the file length. That page's owner depended on
  the store's size: between 1.2.11 and 1.3.0 the seeded store grew from
  593,920 to 700,416 bytes (26 pages), and the page at half the file became
  one of `memory_feedback`. The 1.3.0 store checks its memory tables when it opens,
  so the case now reads: open `error` (`store is corrupt: memory table
  memories is damaged`), retrieval skipped, nothing recovered.
- Measured, not checked: bytes per record 3,031 to 3,277 (empty database
  442,368 to 536,576 bytes, from schema added since 1.2.11); bytes per
  scaled memory 1,310 / 1,312 to 1,314; retrieval latency, median / p95,
  48 memories 4.35 / 6.54 ms and 2.67 / 4.64 ms, 5,048 memories
  12.04 / 30.64 ms and 11.65 / 17.39 ms (index=none, then index=local). These were taken with the load gate off on a shared host at a
  1-minute load near 25, so they are not comparable with the 1.2.11 figures.

## Judging Memory 2.0

- Run `python3 evals/memory_bench.py run --out <dir>` on the change and
  `python3 evals/memory_bench.py compare <baseline results.json> <dir>/results.json`
  to put every metric side by side; `--repo <checkout>` or `--driver <binary>`
  measures another tree.
- Keep `bench/memory-set/v1` and its labels fixed, so results stay comparable.
  New scenarios go in a new set version, with this baseline re-run on it.
- The driver's plan and report formats are the interface. A change to the
  memory API updates the driver, not the dataset or the metrics.
- After an intended change to the deterministic metrics, record a new baseline
  with `--write-baseline` and state the commit here.
- Consolidation (`MemoryService::consolidate`, the operator's
  `consolidate_memories`) runs on demand, not on admission, so the default
  run leaves it out. `run --consolidate` merges near-duplicates after seeding
  and before ranking, the way an operator's run would; compare it with a
  default run to see what consolidation changes.
