# Memory retrieval: G2 baseline, profile and acceptance

What G2 (retrieval scoring, relevance cutoff, stale demotion, holdout;
mengsig/aether#408) is measured against, how to repeat the measurement, and
the rules that decide whether it passed. The dataset, metrics and recorded
baseline are [BASELINE.md](BASELINE.md)'s; nothing in `bench/memory-set/v1`
changes.

## Commands

Every timing run holds the host CI slot, so other builds cannot skew it.
`T` is the item's build cache; `$CARGO_TARGET_DIR` is `$T/cargo-target`.

```sh
# Build a driver (release) from a tree, then keep a copy per revision:
cargo build --release -p aether-memory --example memory_bench
cp "$CARGO_TARGET_DIR/release/examples/memory_bench" "$T/memory_bench-<rev>"

# Five benchmark runs and one retrieval profile, in one slot:
sh scripts/ci_slot.sh sh evals/memory_bench_series.sh "$T/memory_bench-<rev>" <out-dir> 5
# A driver older than --profile (64873e19) profiles with a build of the same code:
PROFILE_DRIVER="$T/memory_bench-profile-ca5b8a88" sh scripts/ci_slot.sh \
  sh evals/memory_bench_series.sh "$T/memory_bench-baseline-64873e19" <out-dir> 5

# Every metric side by side, one run against another:
python3 evals/memory_bench.py compare <before>/run1/results.json <after>/run1/results.json
# The profile alone:
python3 evals/memory_bench.py profile --driver "$T/memory_bench-<rev>" --out <dir>
```

`memory_bench_series.sh` writes `<out-dir>/run<i>/results.json` and
`report.md` per run, `<out-dir>/profile/{profile.json,profile.md,profile-raw.json}`,
and `<out-dir>/series.json` with every run's latency, its median and its
relative range. Each run uses the harness defaults: both arms (`index=none`,
the shipped default, and `index=local`), 30 timed retrievals per query after
one warm-up, 26 queries, `limit` 6, `memory_max` 2,000, and a scaled store of
the 48 dataset memories plus 5,000 seeded filler memories (fixed LCG seed).

## Baseline (before G2)

- **System under test:** the memory code of `64873e19`, the #414 head G2
  starts from. The driver was built there (`memory_bench-baseline-64873e19`,
  sha256 `174a8d0e…08870ba`); the runs ran with the tree at `3f8d1276`, which
  adds only the profiler and these scripts (no file outside
  `crates/aether-memory/examples/` and `evals/` differs from `64873e19`).
- **Host:** Intel Core 5 210H, 12 CPUs, 15 GB; Rust 1.98.1, release build;
  bundled SQLite (rusqlite 0.41.1); Python 3.14.7. Runs held the CI slot;
  the 1-minute load still rose from 2.3 to 8.9 during them, from builds that
  do not take the slot.
- **Deterministic metrics:** identical in all five runs and equal to
  `baseline.json` everywhere except one recovery probe: with V30's tables the
  `garbled_every_8th_page` copy now opens and fails on retrieval (`ok` /
  `error`) instead of failing to open (`error` / `skipped`). It is a page-layout
  effect of G1's migration, not ranking, and V31 may move it again; it is not
  a G2 criterion.

| Metric (5 runs) | index=none | index=local |
|---|---|---|
| precision@3 | 0.2564 | 0.2949 |
| precision@6 | 0.1603 | 0.1731 |
| Tokens injected on the 3 queries with nothing relevant | 305.0 | 306.6667 |
| Tokens injected per turn, mean | 323.0385 | 320.1923 |
| Retrieval p95, 48 memories, per run (us) | 3212, 3231, 7085, 3194, 3253 | 3403, 13972, 4148, 3372, 3601 |
| — median of runs / undisturbed spread | **3231** / 3194–3253 (1.8%) | **3601** / 3372–3601 (6.8%) |
| Retrieval median, 48 memories, median of runs (us) | 2314 | 2469 |
| Retrieval p95, 5,048 memories, per run (us) | 12869, 27047, 25980, 13202, 19974 | 13926, 39481, 15182, 14106, 13937 |
| — median of runs | 19974 | 14106 |

One run in five per arm (none run 3, local run 2) was disturbed by outside
load: its median as well as its p95 doubled. The scaled store's p95 is far
noisier than the small store's, so it is a guard, not a criterion.
`baseline.json`'s own p95 (recorded at `b0ffd05e`) is 3133 us (none) and
3205 us (local).

## Profile: where retrieval spends its time

`python3 evals/memory_bench.py profile`, same stores, 30 passes per query
(`index=none`; `index=local` adds a semantic pass of 4–7% and is otherwise
the same):

| Stage | 48 memories: median us / share | 5,048 memories: median us / share |
|---|---|---|
| Alias resolution (2 reads) | 8 / 0.4% | 11 / 0.1% |
| Structural pages (5 scopes) | 229 / 10.2% | 1219 / 16.0% |
| **Lexical FTS pages (5 scopes)** | **1940 / 85.6%** | **7024 / 76.3%** |
| Feedback tallies (1 read) | 17 / 0.9% | 47 / 0.6% |
| Scoring and sorting in memory | 57 / 3.4% | 383 / 7.8% |
| Whole `retrieve` | 2275 | 8905 |
| Context layer (`memory_layer`) | 8 | 10 |

A retrieval makes 13 store reads over 5 scopes (repository, the repository's
notes for the role, role, agent, subtree) and pools 14.5 candidates (60.7 on
the scaled store). The hotspot is the lexical page, about 390 us per scope:
an FTS5 `MATCH` of an OR of every query word, ranked by `bm25`, filtered by
scope and by G1's candidate `NOT EXISTS` only after the match, and repeated
once per scope with the same text. Scoring itself costs 3% of the call.

What this means for G2:

- Scoring, the cutoff and the holdout draw are in-memory work on about 15
  candidates: they fit in the +10% budget (about 320 us at p95) if they add
  no store reads per candidate.
- Stale-source demotion must not stat files or run git per candidate in the
  retrieval path; precompute staleness or read it from the record or V31.
- Holdout logging is a write: keep it out of `retrieve`'s timed path, or
  batch it with the turn's own commit, or it will cost more than the budget.
- A larger candidate pool multiplies the lexical stage; if G2 widens the
  pool, one lexical query across all scopes instead of one per scope is the
  measured lever to pay for it.

## G2 acceptance rules

Fixed before optimisation. "After" is `memory_bench_series.sh` on the G2
head with 5 runs, no `--check` (the deterministic metrics are meant to move).

1. **Precision:** `precision@3` and `precision@6` strictly above
   `baseline.json` (none 0.2564 / 0.1603, local 0.2949 / 0.1731) in both arms.
   Deterministic: one run decides, every run must agree.
2. **Irrelevant tasks:** `injection.tokens_on_queries_without_relevant`
   strictly below 305.0 (none) and 306.6667 (local).
3. **Latency:** the median over the 5 runs of `latency_us.p95` (48
   memories) at most 1.10 × this baseline: **3554 us** for none (3231 ×
   1.10) and **3961 us** for local (3601 × 1.10). Medians of 5 tolerate up to
   two disturbed runs. If the verifier's host differs, rerun this baseline's
   driver in the same slot session and use its median of 5 in place of 3231 and
   3601. The undisturbed spread is 1.8% (none) and 6.8% (local), well below the
   10% budget.
4. **Guards, reported, not criteria:** the median of runs of
   `latency_us.median` should stay within +10% (2545 us none, 2716 us local);
   `scaled_p95` within +25% of its median of runs, given its noise; recall@k
   and MRR should not fall, and `harmful_exposure_rate` should not rise.
   Explain any guard that moves.
5. **Holdout and migration:** proved by tests, not by the harness: rate 0
   (never held out), rate 1 (always held out, logged), a fixed seed
   (reproducible draws), and V31 over a V30 store keeping every row and its
   usefulness.

A run is disturbed when its retrieval median is more than 25% above the
lowest median of the series; report it, and rerun the series if two or more
runs of an arm are disturbed.
