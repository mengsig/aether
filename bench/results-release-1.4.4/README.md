# Release check 1.4.4: full bench vs v1.4.3

**RESULT: `INCONCLUSIVE (gate passed on an off-target set)`, the good outcome.
Haiku PASS, Luna PASS.**

The command exits nonzero on this set by design (`ALLOW_OFF_TARGET=1`; see
[docs/release-bench.md](../../docs/release-bench.md)). All 660 planned trials
ran: 0 infrastructure errors, 0 harness failures, 0 unpriced trials, 0
timeouts, USD 6.94 API-equivalent.

**CI: passed.** `CARGO_BUILD_JOBS=6 make ci` passed on the benched head
(`CI passed: lint test binary`; nextest 6895 tests, 6895 passed, 2 flaky that
passed on their retry, 48 skipped).

## Gate: v1.4.4 against v1.4.3

| Tier | Pass diff new-old [one-sided 95 % bound] | Cost new/old | McNemar old-only / new-only | p | Gate |
| --- | --- | --- | --- | ---: | --- |
| Haiku | -4.5 pts [-9.1, -0.9] | x0.93 [x0.83, x1.03] | 6 / 1 | 0.125 | PASS |
| Luna | -2.7 pts [-10.9, +4.6] | x1.06 [x0.92, x1.22] | 15 / 12 | 0.701 | PASS |

A tier fails when, at one-sided 95 % confidence, the candidate is more than 5
points below the old release or costs more than 15 % more. Neither tier meets
either condition.

| Arm | Passed | Pass rate | Cost total (USD) | Cost / trial | Trial minutes (sum) | Median s |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| haiku/native | 105 / 110 | 95 % | 1.655 | 0.0150 | 111.0 | 38 |
| haiku/old | 108 / 110 | 98 % | 1.993 | 0.0181 | 131.1 | 44 |
| haiku/new | 103 / 110 | 94 % | 1.850 | 0.0168 | 134.9 | 43 |
| luna/native | 43 / 110 | 39 % | 0.398 | 0.0036 | 95.2 | 45 |
| luna/old | 50 / 110 | 45 % | 0.505 | 0.0046 | 118.9 | 50 |
| luna/new | 47 / 110 | 43 % | 0.537 | 0.0049 | 121.4 | 50 |

## The Haiku drop

The bootstrap interval for Haiku, [-9.1, -0.9], excludes zero. The exact
McNemar test on the same pairs does not (6 old-only, 1 new-only, p = 0.125).
With only seven discordant tasks, the paired bootstrap over tasks is the less
reliable of the two. The evidence points to noise, not a change in 1.4.4.

Tasks that passed on v1.4.3 and failed on v1.4.4 (Haiku):

| Task | Checks passed on new | Check that failed on new | Native Haiku |
| --- | --- | --- | --- |
| exporters-1 | 5/6 | `test_every_exporter_goes_through_format_row` | pass |
| exporters-6 | 5/6 | `test_exporters_format_no_field_themselves` | fail (5/6) |
| exporters-8 | 5/6 | `test_every_exporter_goes_through_format_row` | fail (5/6) |
| query-1 | 11/12 | `test_negation` | pass |
| query-10 | 11/12 | `test_comparisons` | pass |
| versions-9 | 7/11 | four range checks | pass |

Only `ledger-9` went the other way (v1.4.3 9/11, v1.4.4 pass).

- **Three of the six are `exporters`.** It is the one family where the same
  model flips between runs. Native Haiku passes 5 of its 8 tasks, and fails
  `exporters-6` and `-8` the same way as the candidate. In the
  [1.4.3 release bench](../../docs/release-bench/1.4.3.md), v1.4.3 itself
  passed 3 of 8 `exporters` tasks and failed `exporters-1` and `-8`. In this
  run the same v1.4.3 binary passes 7 of 8, `exporters-1` and `-8` included.
  That swing on one unchanged binary (4 tasks) is larger than the new-old gap
  here (3 tasks). Both failed checks are structural (the exporter must call
  `format_row` and not format a field itself), and each new trial still
  passed 5 of 6 checks.
- **The other three are single trials in different families.** `query-1` and
  `query-10` each miss one of 12 checks, and native Haiku passes both.
  `versions-9` misses four range checks.
- Each task ran once per arm (one repeat), so no task shows a repeated failure.

Tasks that passed on v1.4.3 and failed on v1.4.4 (Luna, 15): batching-7,
bucket-5, config-1, duration-1, duration-2, duration-4, duration-8,
exporters-1, ledger-2, pricing-1, pricing-2, pricing-4, pricing-8,
protocol-6, versions-1. The other way (12): bucket-0, diff-5, duration-3,
duration-5, duration-7, exporters-0, exporters-3, ledger-0, ledger-4,
pricing-6, pricing-9, ranges-8. The flips are spread over many templates in
both directions. The largest clusters flip both ways too: `duration` (4
old-only, 3 new-only) and `pricing` (4 old-only, 2 new-only).

### Could 1.4.3..HEAD touch claude-code or codex runs?

Not in a way that can change what the model does:

- **The antigravity adapter (#468)** adds a new provider, engine and models.
  Nothing a Claude Code or Codex agent receives (prompt, harness tool text,
  CLI arguments) is built from the provider list. `prefer_original` changes
  only `antigravity/...` secondary models. In the bench sandbox agy is on
  `PATH` but signed out, so it is unusable there.
- **Startup ordering (ebdf825e8, 63d005115)** changes when antigravity is
  probed: after Claude Code and Codex are detected, not alongside them.
  Detection of those two engines is unchanged, and it now finishes as soon as
  they are known. The agy probe that follows only costs a few seconds of CPU
  for one short `agy` process.
- **Ownership (#469)** moves the claude-code and codex evidence rule into
  `validate()` unchanged and adds a rule for antigravity. Only the error type
  of a refused record changed.
- **`provider_usage`** changes only a test's expectations. `detect.rs` adds a
  `SignedOut` error that only antigravity detection returns.

The cost side agrees: Haiku new is cheaper than old (x0.93), and Luna's cost
ratio (x1.06) is within noise.

## Release contents (v1.4.3..HEAD)

- #468: the antigravity engine over `agy acp`, and the Google models.
- #469: accept antigravity session-ownership evidence (every real antigravity
  task failed with "invalid engine session ownership evidence" without it).
- Fixes to #468 found by this release's `make ci`:
  - 7ef588579: a signed-out agy is reported as not signed in. It was reported
    as "upgrade agy", with agy's progress line pasted in. This broke a doctor
    test on any host with a signed-out agy, and a provider-usage test that
    missed the new engine.
  - ebdf825e8: antigravity is probed after startup detection. Before, every
    daemon start on a machine with agy waited about 2 s for `agy models`
    before scheduling anything. The release process-budget test failed 5 of 5
    times on it (replay 1.7-1.9 s against its 1 s budget). With the fix it
    passed 3 of 3 (0.01-0.2 s).
  - 63d005115: that probe stops when the daemon shuts down, so it never holds
    the store past the final sync's deadline.

## How it was run

- Candidate: release build of `63d005115c6c56f7b56447ac6822e4654ffce894`
  (tree `cc9ed8f61b3f049c55b88667332b5146f6ba097a`), `aether 1.4.4`, binary
  SHA-256 `65d49a77067ef75eda30050022279e6e71e29e0524d366a0109966489093af38`.
  The commits after it on the branch add only this directory.
- Old: Aether v1.4.3 (`bf186083c880e3c9f55d6df7028dabfc658d1de9`). The
  published `aether-x86_64-unknown-linux-musl.tar.gz` was downloaded into an
  empty directory and matched the release `SHA256SUMS`
  (`25cf7df3db41fdab660fc5a5781e82f0f7b4de34990e945dbf62778a8846de5f`). It
  reports `aether 1.4.3`, binary SHA-256
  `3a795ff2a718e8c227ac66ad6bbefd2cfe1838bfcbf77b37eac10e56ca3cc923`. The local
  `v1.4.3` tag names an unrelated release; it was not changed.
- Claude Code 2.1.332, Codex 0.172.0. Set `bench/release-set/v2` (110 tasks,
  sha256 `7f3166c96e35e2ad`), all six arms, one repeat, 600 s timeout,
  `JOBS=6`, `LOAD_MAX=16`.
- Each invocation ran in a private sandbox: a fresh `HOME` holding copies of
  the Claude Code and Codex credentials (mode 600), its own XDG directories,
  `TMPDIR` and work root. The operator daemon's `AETHER_*` variables were
  unset. The sandbox and the copied credentials were deleted when each
  invocation ended. Both invocations ran inside the host CI slot; load stayed
  about 3 against `LOAD_MAX=16`.

```sh
make build
# in the sandbox (HOME, XDG_*, TMPDIR, RELEASE_BENCH_WORK_ROOT private)
sh scripts/ci_slot.sh make bench-release OLD=<verified v1.4.3 aether> ALLOW_OFF_TARGET=1 \
  ARMS=all BUDGET=20 EXTRA=--run-id=rel-1-4-4
sh scripts/ci_slot.sh make bench-release OLD=<verified v1.4.3 aether> ALLOW_OFF_TARGET=1 \
  ARMS=all BUDGET=40 EXTRA=--run-id=rel-1-4-4
```

The first invocation (2026-11-26 23:32 to 2026-11-27 01:49 UTC, 137.3 min) ran
655 of 660 trials at the default `BUDGET=20` and spent USD 6.82. It stopped
before the last block, `workstreams-8`. That is the delegation task, and its
worst-case reservation (several agents at once) did not fit under USD 20.
Its verdict was INCONCLUSIVE because the matrix was unfinished. Haiku was
-4.6 pts [-8.3, -0.9], cost x0.93, 6/1. Luna was -2.8 pts [-10.1, +5.5], cost
x1.06, 15/12.

The orchestrator approved a resume of the same run with `BUDGET=40`: the
budget is not part of a run's resume configuration. The resume (02:21 to
02:22 UTC) ran only the five missing `workstreams-8` trials, all of which
passed, for USD 0.12. The 1.4.3 release bench used `BUDGET=40` for the same
reason. No prompts, checks, set, price table or thresholds were changed.

## make ci

`CARGO_BUILD_JOBS=6 make ci` on `63d005115` (tree `cc9ed8f61b3f`): **passed**.
Nextest ran 6895 tests: 6895 passed, 48 skipped. Two were flaky and passed on
retry: `cli_regressions memory_v2_cli_tests::export_writes_the_artifact_whole_and_import_sends_it_back_as_a_dry_run`
and `cli_regressions command_polish::malformed_config_fails_auto_start_before_creating_the_store`.
The release build, e2e, the 9.8 budgets and process measurements, the
release install check and the TUI captures all passed.

Earlier runs on this branch failed, and their failures led to the fixes above:

- The first, on the version bump, failed `provider_usage` and `config_errors`
  (7ef588579).
- The second failed `measured_process_budgets` 5 of 5 in isolation
  (ebdf825e8).
- The third, after merging #469, failed `shutdown_timeout` on both of its
  tries. It passed 5 of 5 in isolation, and 63d005115 removes the cause it
  exposed.
- While checking that fix, a targeted run of the shutdown and startup tests
  failed `startup_probes hello_is_answered...` once under parallel load
  ("hello took 5.4 s, of which the host stalled 2.1 s"). It passed 5 of 5 in
  isolation, and the final `make ci` passed it.

## Full report

The harness's `report.md` for the run, after the resume:

```text
# Release bench: INCONCLUSIVE (gate passed on an off-target set)

- Task set: `bench/release-set/v2` (110 tasks, sha256 `7f3166c96e35e2ad`) **(off target: not a release verdict)**
- Old: `3a795ff2a718e8c2`, candidate: `65d49a77067ef75e`
- Native CLIs: claude 2.1.332 (Claude Code), codex codex-cli 0.172.0
- Arms: haiku/native, haiku/old, haiku/new, luna/native, luna/old, luna/new; repeats 1; jobs 6; timeout 600s
- Trials: 660; priced spend $6.94 API-equivalent
- Wall time this invocation: 1.3 min

## Arms

| Arm | Trials / planned | Infra errors | Harness failures | Pass rate [95% CI] | Check fraction | Cost / trial | Total cost | Median time |
|---|---:|---:|---:|---|---:|---:|---:|---:|
| haiku/native | 110 / 110 | 0 | 0 | 95% [91%, 99%] | 99% | $0.015 | $1.655 | 38s |
| haiku/new | 110 / 110 | 0 | 0 | 94% [89%, 97%] | 99% | $0.017 | $1.850 | 43s |
| haiku/old | 110 / 110 | 0 | 0 | 98% [95%, 100%] | 100% | $0.018 | $1.993 | 44s |
| luna/native | 110 / 110 | 0 | 0 | 39% [30%, 48%] | 80% | $0.004 | $0.398 | 45s |
| luna/new | 110 / 110 | 0 | 0 | 43% [34%, 52%] | 81% | $0.005 | $0.537 | 50s |
| luna/old | 110 / 110 | 0 | 0 | 45% [35%, 55%] | 85% | $0.005 | $0.505 | 50s |

## Release gate

### haiku: **PASS**

- **candidate vs old** (haiku/new vs haiku/old, 110 tasks, 110 pairs): pass -4.5% [-9.1%, -0.9%], check fraction -0.8% [-1.6%, -0.1%], cost x0.93 [x0.83, x1.03]; McNemar old-only 6 / new-only 1, p = 0.125
- Infra error rate: haiku/old 0.0%, haiku/new 0.0%

### luna: **PASS**

- **candidate vs old** (luna/new vs luna/old, 110 tasks, 110 pairs): pass -2.7% [-10.9%, +4.6%], check fraction -3.8% [-7.7%, -0.1%], cost x1.06 [x0.92, x1.22]; McNemar old-only 15 / new-only 12, p = 0.701
- Infra error rate: luna/old 0.0%, luna/new 0.0%

## Drift reference (Aether old release vs the native CLI)

- **haiku** (haiku/old vs haiku/native, 110 tasks, 110 pairs): pass +2.7% [-1.8%, +7.3%], check fraction +0.3% [-0.4%, +1.1%], cost x1.20 [x1.07, x1.36]; McNemar old-only 2 / new-only 5, p = 0.453
- **luna** (luna/old vs luna/native, 110 tasks, 110 pairs): pass +6.4% [-3.6%, +16.4%], check fraction +4.5% [+0.2%, +9.0%], cost x1.27 [x1.11, x1.45]; McNemar old-only 12 / new-only 19, p = 0.281

A large change here between two reports means the models or native CLIs moved, not Aether.

## Pass rate by family

| Family | haiku/native | haiku/new | haiku/old | luna/native | luna/new | luna/old |
|---|---:|---:|---:|---:|---:|---:|
| algo | 100% | 100% | 100% | 75% | 75% | 75% |
| delegation | 100% | 100% | 100% | 100% | 100% | 100% |
| docspec | 98% | 93% | 100% | 21% | 31% | 38% |
| edgecase | 95% | 100% | 100% | 48% | 33% | 29% |
| longctx | 100% | 100% | 100% | 0% | 0% | 14% |
| multibug | 100% | 100% | 100% | 100% | 100% | 100% |
| port | 100% | 100% | 100% | 36% | 45% | 64% |
| refactor | 62% | 50% | 88% | 38% | 50% | 38% |
| traceback | 100% | 100% | 80% | 60% | 80% | 60% |
```
