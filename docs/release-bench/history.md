# Release bench history: Aether accuracy from 1.0.0 to 1.2.8

**Verdict: no deterioration found from 1.0.0 to 1.2.8. Haiku is measured
on the full set; Luna only on part of it (83 of 110 tasks, one trial each
instead of two), because the run was stopped when the Codex quota ran out.**
Neither test the verdict rests on finds a decline on either tier: the trend
of pass rate over release order is not significantly negative, and 1.2.8 is
not significantly below 1.0.0:

| Tier | Tasks x trials | 1.0.0 pass | 1.2.8 pass | 1.2.8 - 1.0.0 [95% CI] | One-sided McNemar p | Trend, log-odds per version [95% CI] | One-sided trend p |
| --- | --- | ---: | ---: | --- | ---: | --- | ---: |
| Haiku | 110 x 1 | 96.4% | 94.5% | -1.8 [-6.4, +2.7] | 0.344 | -0.075 [-0.206, +0.056] | 0.130 |
| Luna (partial) | 83 x 1 | 53.0% | 47.0% | -6.0 [-16.9, +4.8] | 0.202 | -0.012 [-0.085, +0.060] | 0.369 |

"Not significant" is not "no change": the data bound how large a decline
could be hiding. On Haiku the fitted trend line, spaced by every release,
moves -2.2 points from 1.0.0 to 1.2.8 (95% interval -5.0 to +0.7), and
1.2.8 against 1.0.0 directly is -1.8 points (-6.4 to +2.7). Haiku's pass rate
stays between 93.6% and 96.4% on every release run. Luna's interval is wide:
-1.8 points along the trend line (-10.8 to +7.1) and -6.0 points directly
(-16.9 to +4.8), so this run cannot rule out a Luna loss of 10 points or
more since 1.0.0. It shows no steady decline either: Luna's pass rate moves
between 45.8% and 55.4% with no order (1.0.5, 1.1.1 and 1.2.8 sit at 47.0%,
1.2.7 is the highest at 55.4%). The permutation test agrees with the primary
test on every row.

The chained per-release deltas that prompted this page (Luna 1.2.7 -8.2 and
1.2.8 -5.5) do not add up to a drift here: on the same tasks, 1.2.7 is the
best Luna release of the nine and 1.2.8 sits inside the spread of the older
releases (45.8% to 53.0%). No release stands out as a drop: no version
differs significantly from 1.2.8 (smallest two-sided McNemar p 0.230 on
Luna, 0.688 on Haiku).

## Why one run

Each [per-release page](../README.md#benchmark-reports) compares one
release with the one before it, in its own run. Chained, those comparisons cannot show a
slow drift: every step is within set v2's run-to-run noise (the
[A/A check](../release-bench-2026-10-18-aa-v2.md) measured Luna at -10.0
points between two copies of one binary), and the noise of a chain grows with
its length. Run `hist1` instead puts nine releases side by side as arms of
one run per tier (`hist1-haiku`, `hist1-luna`), on the same 110 tasks, with
the same seed, the same engine CLIs and the same per-trial settings, so every
release is measured under the same conditions at the same time.

## What ran

| Version | Commit | Release archive checked against `SHA256SUMS` | aether sha256 |
| --- | --- | --- | --- |
| 1.0.0 | `435813dd` (tag `v1.0.0`) | yes | `0806654c9d320614` |
| 1.0.5 | `665a7d79` (tag `v1.0.5`) | yes | `b09e68c88886ad0f` |
| 1.0.10 | `15c8792f` (tag `v1.0.10`) | yes | `2f85c9b72d190a01` |
| 1.1.1 | `7842db7f` (tag `v1.1.1`) | yes | `e8258e313f4ab933` |
| 1.2.0 | `78faf9c1` (tag `v1.2.0`) | yes | `36cee796b1524c19` |
| 1.2.3 | `7a5b8534` (tag `v1.2.3`) | yes | `d2b034481e2f97a9` |
| 1.2.6 | `ba4feeac` (tag `v1.2.6`) | yes | `c33a921d256f7288` |
| 1.2.7 | `d357fd4c` (tag `v1.2.7`) | yes | `04888830d2ad24f5` |
| 1.2.8 | `8ce219e7` (tag `v1.2.8`) | yes | `72a80507e6de6e86` |

Every binary is the `aether-x86_64-unknown-linux-musl` asset of its GitHub
release. All nine releases ran: the bench drives 1.0.0 as it drives 1.2.8,
with no change to how it talks to a daemon. The only harness change is
`--release TAG=PATH` (its own commit), which adds arms such as
`haiku/v1.0.0` that run another release binary exactly as the `old` arm runs
its binary: same condition, caps, spend allowance and pricing. A run that
names no release arm is unchanged, so a per-release run still runs as before.

- Set `bench/release-set/v2` (110 tasks, sha256 `7f3166c96e35e2ad`), seed 0,
  timeout 600 s, jobs 6, load-max 16, `--allow-off-target`: the per-trial
  settings of [1.2.8](1.2.8.md).
- Haiku: one trial per task and version (990 trials), run `hist1-haiku`.
  Luna: planned at two repeats per task and version (1,980 trials), run
  `hist1-luna`, because its noise is larger; it was stopped at 754 trials
  ([what is missing](#luna-what-is-missing-and-why)).
- Both runs: `--old` 1.2.7, `--new` 1.2.8, and the seven older releases
  through `--release`. The bench runs every arm of one (task, repeat) block
  next to each other in a shuffled order, so all versions of a task ran
  within minutes of each other.

The command, per tier (`haiku` with `--repeats 1`, `luna` with `--repeats 2`):

```
python3 evals/release_bench.py run --set bench/release-set/v2 \
  --old <aether 1.2.7> --new <aether 1.2.8> \
  --arms haiku/old,haiku/new,haiku/v1.0.0,haiku/v1.0.5,haiku/v1.0.10,haiku/v1.1.1,haiku/v1.2.0,haiku/v1.2.3,haiku/v1.2.6 \
  --release v1.0.0=<aether 1.0.0> --release v1.0.5=<aether 1.0.5> ... --release v1.2.6=<aether 1.2.6> \
  --jobs 6 --load-max 16 --budget <cap> --repeats 1 --allow-off-target --run-id hist1-haiku
```

The tables below come from
`python3 evals/release_bench_history.py report docs/release-bench/history/hist1-haiku.csv docs/release-bench/history/hist1-luna.csv --positions v1.0.0=0,v1.0.5=5,v1.0.10=10,v1.1.1=12,v1.2.0=13,v1.2.3=16,v1.2.6=19,v1.2.7=20,v1.2.8=21`,
and the CSVs are the runs' per-trial records, exported with
`release_bench_history.py export` (`old` and `new` renamed to v1.2.7 and
v1.2.8; Luna's with `--partial`).

### Comparability

The engine CLIs are today's for every arm: Claude Code 2.1.316 and Codex
0.166.0, in a private home holding only copies of their logins. The models
behind the tiers are today's too. So this run measures Aether itself (its
tools, prompts, scheduling and bridges across releases), not the engines or
models over time: a release from October ran here against the engines of
November, not the ones it shipped with. An old release that relied on an
older engine's behaviour would show here as a loss of that old release, not
of 1.2.8.

### Statistics

- Pass rate: share of trials that passed every check. A harness failure (the
  Aether daemon did not stop within 10 s after the trial, or the engine never
  started) counts as a failed trial, as in the release gate; the graded pass
  rate counts it by its checks instead. Infra errors (the bench's own steps,
  provider outages) are left out; there were none.
- Intervals on a pass rate resample tasks (4,000 bootstrap draws), with a
  task's repeats averaged first.
- Each version against 1.2.8 pairs the same (task, repeat) cells: exact
  McNemar test on the discordant pairs, two-sided and one-sided for "1.2.8
  worse", and a task bootstrap 95% interval on the difference.
- Trend: two tests of pass on release order. The primary test is a
  conditional logistic regression (task fixed effect, exact conditional
  likelihood); its slope is in log-odds per
  step, with a Wald 95% interval and a likelihood-ratio p. The check beside
  it is a permutation test that shuffles version labels within each (task,
  repeat); its slope is a within-task least-squares slope in pass-rate points
  per step, with a task bootstrap interval. Both run with one step per
  version, on the recorded and on the graded pass; both run again on the
  recorded pass spaced by every release since 1.0.0 (1.0.5 is release 5,
  1.0.10 is 10, 1.1.1 is 12, 1.2.0 is 13, 1.2.3 is 16, 1.2.6 is 19, 1.2.7 is
  20, 1.2.8 is 21).
- "Trend significantly negative" means the primary test's one-sided p for a
  decline is below 0.05; the permutation test's answer is shown beside it,
  and the verdict says so if the two disagree. "Significantly below" means a
  one-sided McNemar p below 0.05. One-sided tests make a decline easier to
  find, not harder.

## Results

The report of `release_bench_history.py`, unedited. "Pairs" counts matched
(task, repeat) cells; a positive difference means 1.2.8 passed more.

### Haiku

| Version | Trials | Harness failures | Infra errors | Pass rate [95% CI] | Graded pass rate | Cost / trial | Median time |
| --- | ---: | ---: | ---: | --- | ---: | ---: | ---: |
| v1.0.0 | 110 | 0 | 0 | 96.4% [92.7%, 99.1%] | 96.4% | $0.0192 | 40s |
| v1.0.5 | 110 | 0 | 0 | 95.5% [90.9%, 99.1%] | 95.5% | $0.0192 | 37s |
| v1.0.10 | 110 | 0 | 0 | 94.5% [90.0%, 98.2%] | 94.5% | $0.0207 | 40s |
| v1.1.1 | 110 | 0 | 0 | 96.4% [92.7%, 99.1%] | 96.4% | $0.0186 (1 unpriced) | 39s |
| v1.2.0 | 110 | 1 | 0 | 94.5% [90.0%, 98.2%] | 94.5% | $0.0212 | 39s |
| v1.2.3 | 110 | 0 | 0 | 95.5% [90.9%, 99.1%] | 95.5% | $0.0167 | 37s |
| v1.2.6 | 110 | 0 | 0 | 93.6% [89.1%, 98.2%] | 93.6% | $0.0177 | 40s |
| v1.2.7 | 110 | 1 | 0 | 93.6% [89.1%, 98.2%] | 94.5% | $0.0160 | 37s |
| v1.2.8 | 110 | 2 | 0 | 94.5% [90.0%, 98.2%] | 96.4% | $0.0217 | 38s |

#### Haiku: each version against v1.2.8

| Version | Pairs | Version pass | v1.2.8 pass | Version-only / v1.2.8-only | McNemar p | One-sided p (v1.2.8 worse) | v1.2.8 - version [95% CI] |
| --- | ---: | ---: | ---: | --- | ---: | ---: | --- |
| v1.0.0 | 110 | 96.4% | 94.5% | 4 / 2 | 0.688 | 0.344 | -1.8 [-6.4, +2.7] |
| v1.0.5 | 110 | 95.5% | 94.5% | 6 / 5 | 1.000 | 0.500 | -0.9 [-7.3, +4.5] |
| v1.0.10 | 110 | 94.5% | 94.5% | 5 / 5 | 1.000 | 0.623 | +0.0 [-5.5, +5.5] |
| v1.1.1 | 110 | 96.4% | 94.5% | 6 / 4 | 0.754 | 0.377 | -1.8 [-7.3, +3.6] |
| v1.2.0 | 110 | 94.5% | 94.5% | 4 / 4 | 1.000 | 0.637 | +0.0 [-5.5, +5.5] |
| v1.2.3 | 110 | 95.5% | 94.5% | 6 / 5 | 1.000 | 0.500 | -0.9 [-7.3, +4.5] |
| v1.2.6 | 110 | 93.6% | 94.5% | 4 / 5 | 1.000 | 0.746 | +0.9 [-4.5, +6.4] |
| v1.2.7 | 110 | 93.6% | 94.5% | 5 / 6 | 1.000 | 0.726 | +0.9 [-4.5, +7.3] |

#### Haiku: trend over release order

- **Pass rate as recorded.** Conditional logit (task fixed effect, 26 informative tasks): -0.075 log-odds per version step [95% CI -0.206, +0.056], odds ratio x0.928; likelihood-ratio p = 0.260 two-sided, 0.130 one-sided for a decline.
  Within-task permutation test: -0.3 points per version step [bootstrap 95% CI -0.7, +0.1]; p = 0.278 two-sided, 0.139 one-sided for a decline.
- **Graded pass rate (a harness failure counted by its grade).** Conditional logit (task fixed effect, 24 informative tasks): -0.028 log-odds per version step [95% CI -0.163, +0.106], odds ratio x0.972; likelihood-ratio p = 0.681 two-sided, 0.341 one-sided for a decline.
  Within-task permutation test: -0.1 points per version step [bootstrap 95% CI -0.5, +0.3]; p = 0.706 two-sided, 0.356 one-sided for a decline.
- **Pass rate as recorded, spaced by every release since the oldest.** Conditional logit (task fixed effect, 26 informative tasks): -0.031 log-odds per release [95% CI -0.084, +0.021], odds ratio x0.969; likelihood-ratio p = 0.237 two-sided, 0.119 one-sided for a decline.
  Within-task permutation test: -0.1 points per release [bootstrap 95% CI -0.2, +0.0]; p = 0.245 two-sided, 0.124 one-sided for a decline.

- Deterioration test (one-sided, alpha 0.05 each): trend significantly negative by the conditional logit (the primary test): **no** (permutation test: no); v1.2.8 significantly below v1.0.0 by McNemar: **no**.

### Luna

| Version | Trials | Harness failures | Infra errors | Pass rate [95% CI] | Graded pass rate | Cost / trial | Median time |
| --- | ---: | ---: | ---: | --- | ---: | ---: | ---: |
| v1.0.0 | 83 | 0 | 0 | 53.0% [42.2%, 63.9%] | 53.0% | $0.0047 | 29s |
| v1.0.5 | 83 | 0 | 0 | 47.0% [36.1%, 57.8%] | 47.0% | $0.0047 | 32s |
| v1.0.10 | 83 | 0 | 0 | 53.0% [42.2%, 63.9%] | 53.0% | $0.0054 | 29s |
| v1.1.1 | 83 | 0 | 0 | 47.0% [36.1%, 57.8%] | 47.0% | $0.0049 | 29s |
| v1.2.0 | 83 | 0 | 0 | 49.4% [38.6%, 60.2%] | 49.4% | $0.0049 | 30s |
| v1.2.3 | 83 | 0 | 0 | 49.4% [38.6%, 60.2%] | 49.4% | $0.0051 | 28s |
| v1.2.6 | 83 | 0 | 0 | 45.8% [34.9%, 56.6%] | 45.8% | $0.0052 | 28s |
| v1.2.7 | 83 | 0 | 0 | 55.4% [44.6%, 66.3%] | 55.4% | $0.0046 | 27s |
| v1.2.8 | 83 | 0 | 0 | 47.0% [36.1%, 57.8%] | 47.0% | $0.0046 | 30s |

#### Luna: each version against v1.2.8

| Version | Pairs | Version pass | v1.2.8 pass | Version-only / v1.2.8-only | McNemar p | One-sided p (v1.2.8 worse) | v1.2.8 - version [95% CI] |
| --- | ---: | ---: | ---: | --- | ---: | ---: | --- |
| v1.0.0 | 83 | 53.0% | 47.0% | 14 / 9 | 0.405 | 0.202 | -6.0 [-16.9, +4.8] |
| v1.0.5 | 83 | 47.0% | 47.0% | 10 / 10 | 1.000 | 0.588 | +0.0 [-10.8, +10.8] |
| v1.0.10 | 83 | 53.0% | 47.0% | 18 / 13 | 0.473 | 0.237 | -6.0 [-19.3, +7.2] |
| v1.1.1 | 83 | 47.0% | 47.0% | 9 / 9 | 1.000 | 0.593 | +0.0 [-10.8, +9.6] |
| v1.2.0 | 83 | 49.4% | 47.0% | 12 / 10 | 0.832 | 0.416 | -2.4 [-13.3, +8.4] |
| v1.2.3 | 83 | 49.4% | 47.0% | 12 / 10 | 0.832 | 0.416 | -2.4 [-13.3, +8.4] |
| v1.2.6 | 83 | 45.8% | 47.0% | 10 / 11 | 1.000 | 0.668 | +1.2 [-9.6, +12.0] |
| v1.2.7 | 83 | 55.4% | 47.0% | 16 / 9 | 0.230 | 0.115 | -8.4 [-20.5, +3.6] |

#### Luna: trend over release order

- **Pass rate as recorded.** Conditional logit (task fixed effect, 60 informative tasks): -0.012 log-odds per version step [95% CI -0.085, +0.060], odds ratio x0.988; likelihood-ratio p = 0.739 two-sided, 0.369 one-sided for a decline.
  Within-task permutation test: -0.2 points per version step [bootstrap 95% CI -1.3, +0.9]; p = 0.754 two-sided, 0.383 one-sided for a decline.
- **Graded pass rate (a harness failure counted by its grade).** Conditional logit (task fixed effect, 60 informative tasks): -0.012 log-odds per version step [95% CI -0.085, +0.060], odds ratio x0.988; likelihood-ratio p = 0.739 two-sided, 0.369 one-sided for a decline.
  Within-task permutation test: -0.2 points per version step [bootstrap 95% CI -1.3, +0.9]; p = 0.754 two-sided, 0.383 one-sided for a decline.
- **Pass rate as recorded, spaced by every release since the oldest.** Conditional logit (task fixed effect, 60 informative tasks): -0.006 log-odds per release [95% CI -0.034, +0.022], odds ratio x0.994; likelihood-ratio p = 0.680 two-sided, 0.340 one-sided for a decline.
  Within-task permutation test: -0.1 points per release [bootstrap 95% CI -0.5, +0.3]; p = 0.685 two-sided, 0.350 one-sided for a decline.

- Deterioration test (one-sided, alpha 0.05 each): trend significantly negative by the conditional logit (the primary test): **no** (permutation test: no); v1.2.8 significantly below v1.0.0 by McNemar: **no**.

### Notes on the records

- **Harness failures** (Haiku only; Luna had none): the trial's daemon did
  not stop within 10 s after the trial (`daemon_cleanup_error`), counted as
  a failed trial. 1.2.8 had two (`exporters-5`, `pricing-1`) and 1.2.7 one
  (`bucket-0`), all of whose work passed every check; 1.2.0 had one
  (`ranges-0`) whose work also failed. The graded rows count these by their
  checks: 1.2.8 then reads 96.4%, level with 1.0.0. No infra errors.
- **Cost.** Haiku's cost per trial ranges from $0.0160 (1.2.7) to $0.0217
  (1.2.8); 1.2.8 is the highest, with or without its one $0.065
  `pricing-1` trial ($0.0213 without it). The run's own gate puts 1.2.8
  against 1.2.7 at x1.36 [x1.08, x1.70], where r128 measured x0.84 between
  1.2.7 and the 1.2.8 release head's own build. Cost is not the question
  of this page and was not followed up. Luna costs $0.0046 to $0.0054 per
  trial on every release. One 1.1.1 Haiku trial is unpriced.
- **Where the Haiku failures are.** Of the 26 tasks with any failure, only
  `exporters-0` (fails on every release but 1.2.8), `query-10` (five
  releases), `exporters-8` (five), `exporters-5` and `exporters-6` (four
  each) fail on more than two releases; the rest fail once or twice,
  scattered across releases.

### Luna: what is missing and why

The operator stopped the Luna run on 2026-11-01 when the Codex quota ran out
(about 1% left until 2026-11-04), with 754 of 1,980 planned trials done:

- **The second repeat** did not run at all, so every Luna row is one trial
  per task and version, not the planned two.
- **26 tasks** never ran on any version: `billing-8`, `bucket-7`,
  `config-6`, `cron-0`, `diff-6`, `duration-3`, `duration-4`, `duration-7`,
  `duration-9`, `exporters-0`, `exporters-8`, `ledger-4`, `ledger-9`,
  `pricing-1`, `pricing-8`, `pricing-9`, `protocol-1`, `protocol-3`,
  `protocol-7`, `query-3`, `query-6`, `query-8`, `slug-7`, `versions-1`,
  `versions-4`, `workstreams-8`. A 27th, `slug-4`, ran on seven versions
  but not on 1.0.10 or 1.2.6, so its seven trials are left out too
  (`export --partial` keeps only the tasks every version finished).
- By family, the 83 analysed tasks cover algo 7 of 8 tasks, delegation 3 of 4, docspec 31
  of 42, edgecase 18 of 21, longctx 4 of 7, multibug 3 of 4, port 8 of 11,
  refactor 6 of 8 and traceback 3 of 5. Every version is missing the same
  tasks, and the bench runs the tasks in a seeded order fixed before any
  trial, so which tasks are missing does not depend on any result: the 83
  are a random subset, and every comparison above is paired on them.
- Every release is in the Luna rows; no version was dropped.

A first Luna start was discarded after seven trials: five of them could not
start Codex 0.166.0, which failed to initialise its state database in the
fresh private home (as on `cron-8` in [1.2.8](1.2.8.md)). That start was
stopped and set aside, and the run started again from nothing under the same
run id, with the Codex state that start had created already in place; no
trial of the second start hit the error.

## Chained per-release results, for context

Each row is one per-release run of a release against the one before it, in
points of pass rate, with the bounds and McNemar p each page reports (most
pages give one-sided 95% bounds). Where a page reran a tier, the pooled or
final figure is given and the first run in brackets.

| Release | Against | Haiku | Luna |
| --- | --- | --- | --- |
| [1.2.0](1.2.0.md) | 1.1.1 | +2.7 [-2.7, +8.2], p = 0.581 | pooled -4.5 [-10.5, +0.9] (first run -7.3, p = 0.185) |
| [1.2.1](1.2.1.md) | 1.2.0 | pooled -1.1 [-3.8, +1.4], p = 0.608 (first run -6.4) | final run -4.5 [-12.7, +2.7], p = 0.442 (first run +1.8) |
| [1.2.2](1.2.2.md) | 1.2.1 | +0.0 [-4.5, +4.5], p = 1.000 | +4.5 [-3.6, +12.7], p = 0.487 |
| [1.2.3](1.2.3.md) | 1.2.2 | +0.9 [-2.7, +5.5], p = 1.000 | pooled -2.7 [-8.6, +3.2], p = 0.526 (first run -9.1, p = 0.099) |
| [1.2.4](1.2.4.md) | 1.2.3 | +0.0 [-2.7, +2.7], p = 1.000 | +3.6 [-5.5, +11.8], p = 0.608 |
| [1.2.5](1.2.5.md) | 1.2.4 | +0.0 [-4.5, +4.5], p = 1.000 | -0.9 [-10.0, +7.3], p = 1.000 |
| [1.2.6](1.2.6.md) | 1.2.5 | -1.8 [-5.5, +1.8], p = 0.688 | -0.9 [-9.1, +7.3], p = 1.000 |
| [1.2.7](1.2.7.md) | 1.2.6 | +0.0 [-1.8, +1.8], p = 1.000 | -8.2 [-16.4, +0.9], p = 0.175 |
| [1.2.8](1.2.8.md) | 1.2.7 | +0.9 [-2.7, +4.5], p = 1.000 | r128 -5.5 [-14.5, +3.6], p = 0.392; r128b +4.5 [-4.5, +13.6], p = 0.522; pooled over 220 pairs -0.5 [-6.8, +5.9], p = 1.000 |

Summed, the first-run Luna deltas from 1.2.0 to 1.2.8 come to about -22
points, and the pooled or final ones to about -14. Neither sum estimates
anything: each step carries its own run's noise (about 10 points for Luna on set v2),
and a sum of nine noisy steps carries roughly three times as much. The single
run above measures 1.0.0 and 1.2.8 on the same tasks at the same time
instead, and finds 1.2.8 -6.0 points behind 1.0.0 on Luna, not significant,
and 1.2.7, whose chained step was -8.2, the best of the nine.

## Spend

API-equivalent prices from the run's price table: $18.78 for the 990 Haiku
trials and $3.67 for the 754 Luna trials, $22.45 in all. The bench charges
a trial that a stopped invocation abandoned its full reservation (about
$3.14 on Haiku). The Haiku run was stopped and resumed three times, by an
engine sign-in outage, by its `--budget` cap filling with those charges, and
by a two-hour limit on the invocation, and its cap was raised on the resumes,
finally to $85, only to clear those charges. Luna's cap was $40. Earlier attempts of this
work, lost to an expired login and an engine sign-in outage, were discarded
and are not in these records.

## Run reports

The bench's own reports of the two runs, unedited apart from heading levels.
Their verdict lines judge only `old` (1.2.7) against `new` (1.2.8) and read
INCONCLUSIVE because set v2 is off target (and, for Luna, because the run did
not finish); the history tables above are the result of this page. The Haiku
header's wall time covers the last of its resumed invocations only.

### hist1-haiku

#### Release bench: INCONCLUSIVE (gate passed on an off-target set)

- Task set: `bench/release-set/v2` (110 tasks, sha256 `7f3166c96e35e2ad`) **(off target: not a release verdict)**
- Old: `04888830d2ad24f5`, candidate: `72a80507e6de6e86`
- Release v1.0.0: `0806654c9d320614`
- Release v1.0.10: `2f85c9b72d190a01`
- Release v1.0.5: `b09e68c88886ad0f`
- Release v1.1.1: `e8258e313f4ab933`
- Release v1.2.0: `36cee796b1524c19`
- Release v1.2.3: `d2b034481e2f97a9`
- Release v1.2.6: `c33a921d256f7288`
- Native CLIs: claude 2.1.316 (Claude Code), codex codex-cli 0.166.0
- Arms: haiku/old, haiku/new, haiku/v1.0.0, haiku/v1.0.5, haiku/v1.0.10, haiku/v1.1.1, haiku/v1.2.0, haiku/v1.2.3, haiku/v1.2.6; repeats 1; jobs 6; timeout 600s
- Trials: 990; priced spend $18.78 API-equivalent
- Wall time this invocation: 51.1 min

##### Arms

| Arm | Trials / planned | Infra errors | Harness failures | Pass rate [95% CI] | Check fraction | Cost / trial | Total cost | Median time |
|---|---:|---:|---:|---|---:|---:|---:|---:|
| haiku/new | 110 / 110 | 0 | 2 | 95% [90%, 98%] | 99% | $0.022 | $2.386 | 38s |
| haiku/old | 110 / 110 | 0 | 1 | 94% [89%, 98%] | 99% | $0.016 | $1.755 | 37s |
| haiku/v1.0.0 | 110 / 110 | 0 | 0 | 96% [93%, 99%] | 99% | $0.019 | $2.115 | 40s |
| haiku/v1.0.10 | 110 / 110 | 0 | 0 | 95% [90%, 98%] | 99% | $0.021 | $2.277 | 40s |
| haiku/v1.0.5 | 110 / 110 | 0 | 0 | 95% [91%, 99%] | 99% | $0.019 | $2.107 | 37s |
| haiku/v1.1.1 | 110 / 110 | 0 | 0 | 96% [93%, 99%] | 100% | $0.019 | $2.023 (1 unpriced) | 39s |
| haiku/v1.2.0 | 110 / 110 | 0 | 1 | 95% [90%, 98%] | 99% | $0.021 | $2.336 | 39s |
| haiku/v1.2.3 | 110 / 110 | 0 | 0 | 95% [91%, 99%] | 99% | $0.017 | $1.834 | 37s |
| haiku/v1.2.6 | 110 / 110 | 0 | 0 | 94% [89%, 98%] | 99% | $0.018 | $1.950 | 40s |

##### Release gate

###### haiku: **PASS**

- **candidate vs old** (haiku/new vs haiku/old, 110 tasks, 110 pairs): pass +0.9% [-3.6%, +6.4%], check fraction +0.3% [-0.7%, +1.2%], cost x1.36 [x1.08, x1.70]; McNemar old-only 5 / new-only 6, p = 1.000
- Infra error rate: haiku/old 0.0%, haiku/new 0.0%
- Warning: cost point estimate x1.36 is above x1.15

##### Pass rate by family

| Family | haiku/new | haiku/old | haiku/v1.0.0 | haiku/v1.0.10 | haiku/v1.0.5 | haiku/v1.1.1 | haiku/v1.2.0 | haiku/v1.2.3 | haiku/v1.2.6 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| algo | 100% | 100% | 100% | 100% | 100% | 100% | 100% | 100% | 100% |
| delegation | 100% | 100% | 100% | 100% | 100% | 100% | 100% | 100% | 100% |
| docspec | 95% | 95% | 98% | 90% | 98% | 95% | 98% | 93% | 95% |
| edgecase | 95% | 95% | 100% | 100% | 100% | 100% | 95% | 100% | 100% |
| longctx | 100% | 100% | 100% | 100% | 100% | 100% | 100% | 100% | 86% |
| multibug | 100% | 100% | 100% | 100% | 100% | 100% | 100% | 100% | 100% |
| port | 91% | 100% | 100% | 100% | 100% | 100% | 100% | 100% | 100% |
| refactor | 75% | 50% | 62% | 75% | 62% | 75% | 50% | 75% | 50% |
| traceback | 100% | 100% | 100% | 100% | 80% | 100% | 100% | 100% | 100% |

### hist1-luna

#### Release bench: INCONCLUSIVE

- Task set: `bench/release-set/v2` (110 tasks, sha256 `7f3166c96e35e2ad`) **(off target: not a release verdict)**
- Old: `04888830d2ad24f5`, candidate: `72a80507e6de6e86`
- Release v1.0.0: `0806654c9d320614`
- Release v1.0.10: `2f85c9b72d190a01`
- Release v1.0.5: `b09e68c88886ad0f`
- Release v1.1.1: `e8258e313f4ab933`
- Release v1.2.0: `36cee796b1524c19`
- Release v1.2.3: `d2b034481e2f97a9`
- Release v1.2.6: `c33a921d256f7288`
- Native CLIs: claude 2.1.316 (Claude Code), codex codex-cli 0.166.0
- Arms: luna/old, luna/new, luna/v1.0.0, luna/v1.0.5, luna/v1.0.10, luna/v1.1.1, luna/v1.2.0, luna/v1.2.3, luna/v1.2.6; repeats 2; jobs 6; timeout 600s
- Trials: 754; priced spend $3.67 API-equivalent

##### Arms

| Arm | Trials / planned | Infra errors | Harness failures | Pass rate [95% CI] | Check fraction | Cost / trial | Total cost | Median time |
|---|---:|---:|---:|---|---:|---:|---:|---:|
| luna/new | 84 / 220 | 0 | 0 | 46% [36%, 57%] | 86% | $0.005 | $0.388 | 30s |
| luna/old | 84 / 220 | 0 | 0 | 55% [44%, 65%] | 86% | $0.005 | $0.384 | 27s |
| luna/v1.0.0 | 84 / 220 | 0 | 0 | 52% [42%, 63%] | 84% | $0.005 | $0.389 | 29s |
| luna/v1.0.10 | 83 / 220 | 0 | 0 | 53% [42%, 64%] | 81% | $0.005 | $0.446 | 29s |
| luna/v1.0.5 | 84 / 220 | 0 | 0 | 46% [36%, 57%] | 85% | $0.005 | $0.393 | 31s |
| luna/v1.1.1 | 84 / 220 | 0 | 0 | 46% [36%, 57%] | 87% | $0.005 | $0.411 | 29s |
| luna/v1.2.0 | 84 / 220 | 0 | 0 | 49% [38%, 60%] | 84% | $0.005 | $0.409 | 31s |
| luna/v1.2.3 | 84 / 220 | 0 | 0 | 49% [38%, 60%] | 86% | $0.005 | $0.424 | 27s |
| luna/v1.2.6 | 83 / 220 | 0 | 0 | 46% [35%, 57%] | 85% | $0.005 | $0.427 | 28s |

The planned matrix did not finish, so the run cannot pass:

- luna/new ran 84 of 220 planned trials
- luna/old ran 84 of 220 planned trials
- luna/v1.0.0 ran 84 of 220 planned trials
- luna/v1.0.10 ran 83 of 220 planned trials
- luna/v1.0.5 ran 84 of 220 planned trials
- luna/v1.1.1 ran 84 of 220 planned trials
- luna/v1.2.0 ran 84 of 220 planned trials
- luna/v1.2.3 ran 84 of 220 planned trials
- luna/v1.2.6 ran 83 of 220 planned trials

##### Release gate

###### luna: **INCONCLUSIVE**

- **candidate vs old** (luna/new vs luna/old, 84 tasks, 84 pairs): pass -8.3% [-17.9%, +1.2%], check fraction -0.2% [-4.2%, +3.9%], cost x1.01 [x0.90, x1.14]; McNemar old-only 16 / new-only 9, p = 0.230
- Infra error rate: luna/old 0.0%, luna/new 0.0%
- Inconclusive because: luna/old ran 84 of 220 planned trials
- Inconclusive because: luna/new ran 84 of 220 planned trials
- Warning: pass rate point estimate -8.3% is below -5%

##### Pass rate by family

| Family | luna/new | luna/old | luna/v1.0.0 | luna/v1.0.10 | luna/v1.0.5 | luna/v1.1.1 | luna/v1.2.0 | luna/v1.2.3 | luna/v1.2.6 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| algo | 71% | 100% | 71% | 71% | 100% | 100% | 71% | 71% | 86% |
| delegation | 100% | 100% | 100% | 100% | 100% | 100% | 100% | 100% | 67% |
| docspec | 32% | 39% | 26% | 45% | 26% | 26% | 32% | 29% | 26% |
| edgecase | 37% | 47% | 53% | 50% | 26% | 32% | 58% | 47% | 44% |
| longctx | 25% | 25% | 50% | 0% | 25% | 0% | 0% | 25% | 25% |
| multibug | 100% | 100% | 100% | 100% | 100% | 100% | 100% | 100% | 100% |
| port | 50% | 75% | 75% | 38% | 62% | 75% | 50% | 88% | 62% |
| refactor | 50% | 67% | 67% | 83% | 67% | 67% | 67% | 33% | 67% |
| traceback | 100% | 33% | 100% | 67% | 100% | 67% | 33% | 67% | 33% |
