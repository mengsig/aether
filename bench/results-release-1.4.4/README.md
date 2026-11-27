# Release check 1.4.4: full bench vs v1.4.3

**RESULT: `INCONCLUSIVE (gate passed on an off-target set)`, the good outcome.
Haiku PASS, Luna PASS.** Luna carries a cost warning: its point estimate,
x1.16, is above x1.15, but its one-sided bound, x1.00, is below x1.15, so
it is not a FAIL. See [Cost](#cost) below.

The command exits nonzero on this set by design (`ALLOW_OFF_TARGET=1`; see
[docs/release-bench.md](../../docs/release-bench.md)). All 660 trials of the
final matrix ran: 0 infrastructure errors, 0 harness failures, 0 unpriced
trials, 0 timeouts.

The final candidate is `5250b1c15`. Its routing fix adds a step to the
scheduler that every agent passes through, so its `haiku/new` and `luna/new`
arms were run again on it (run `rel-1-4-4-new2`, 220 trials, USD 2.70). The
native and old arms come from the full run `rel-1-4-4`. Their binaries (the
native CLIs and the published v1.4.3) did not change. The gate below is the
harness's own `report` over the two runs combined (`rel-1-4-4-final`).

**CI: passed.** `CARGO_BUILD_JOBS=6 make ci` passed on `5250b1c15` (tree
`0e6cee9e99da`): `CI passed: lint test binary`; nextest 6901 tests, 6901
passed, 2 flaky that passed on their retry, 48 skipped. Each flaky test then
passed 5 of 5 isolated reruns (see [make ci](#make-ci)).

## Gate: v1.4.4 (5250b1c15) against v1.4.3

| Tier | Pass diff new-old [one-sided 95 % bound] | Cost new/old | McNemar old-only / new-only | p | Gate |
| --- | --- | --- | --- | ---: | --- |
| Haiku | -3.6 pts [-7.3, +0.0] | x1.06 [x0.93, x1.21] | 5 / 1 | 0.219 | PASS |
| Luna | -1.8 pts [-10.0, +7.3] | x1.16 [x1.00, x1.35] | 17 / 15 | 0.860 | PASS, cost warning |

A tier fails when, at one-sided 95 % confidence, the candidate is more than 5
points below the old release or costs more than 15 % more. Neither tier meets
either condition.

| Arm | Passed | Pass rate | Cost total (USD) | Cost / trial | Trial minutes (sum) | Median s |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| haiku/native | 105 / 110 | 95 % | 1.655 | 0.0150 | 111.0 | 38 |
| haiku/old | 108 / 110 | 98 % | 1.993 | 0.0181 | 131.1 | 44 |
| haiku/new | 104 / 110 | 95 % | 2.116 | 0.0192 | 139.2 | 45 |
| luna/native | 43 / 110 | 39 % | 0.398 | 0.0036 | 95.2 | 45 |
| luna/old | 50 / 110 | 45 % | 0.505 | 0.0046 | 118.9 | 50 |
| luna/new | 48 / 110 | 44 % | 0.587 | 0.0053 | 141.6 | 52 |

The first candidate, `63d005115`, also passed on both tiers in the full run:
Haiku -4.5 pts [-9.1, -0.9], x0.93 [x0.83, x1.03], 6/1, p = 0.125; Luna -2.7
pts [-10.9, +4.6], x1.06 [x0.92, x1.22], 15/12, p = 0.701. Its new arms went
103/110 (USD 1.850) and 47/110 (USD 0.537).

## Noise reference: two runs of the new arms

The two candidates differ only in the routing fix and the agy listing fix.
Neither runs for a `claude-code` or `codex` agent: the scheduler step returns
at once for an agent with no `antigravity/...` model. Their new arms ran two
hours apart, so comparing them shows this bench's noise on an unchanged
model path:

| Tier | new on 63d005115 | new on 5250b1c15 | Tasks that flipped between them | Cost ratio |
| --- | --- | --- | --- | --- |
| Haiku | 103 / 110, USD 1.850 | 104 / 110, USD 2.116 | 3 (1 / 2) | x1.14 |
| Luna | 47 / 110, USD 0.537 | 48 / 110, USD 0.587 | 27 (13 / 14) | x1.09 |

Luna's 27 flips between two near-identical candidates is about as many as
between v1.4.3 and v1.4.4 (32). Haiku's cost rise between the two runs comes
mostly from a few long `pricing` trials (`pricing-6` USD 0.031 to 0.113,
`pricing-10` 0.038 to 0.095, `pricing-5` 0.059 to 0.107).

## The Haiku drop

Tasks that passed on v1.4.3 and failed on v1.4.4 (Haiku, final candidate):

| Task | Checks passed on new | Native Haiku | Also failed on 63d005115 |
| --- | --- | --- | --- |
| exporters-1 | 5/6 | pass | yes |
| exporters-6 | 5/6 | fail | yes |
| exporters-7 | 4/5 | pass | no |
| query-1 | 11/12 | pass | yes |
| query-10 | 11/12 | pass | yes |

Only `ledger-9` went the other way.

- **Three of the five are `exporters`**, the family where the same model flips
  between runs. In the [1.4.3 release bench](../../docs/release-bench/1.4.3.md)
  v1.4.3 itself passed 3 of 8 `exporters` tasks, failing `exporters-1` and
  `-8`. In this run the same v1.4.3 binary passed 7 of 8. Between the two
  new-arm runs, `exporters-7` and `exporters-8` swapped.
- **`query-1` and `query-10`** each miss one of 12 checks in both new-arm
  runs. They are the one repeated pair; each is still a single check.
- `versions-9` (7/11 on the first candidate) passed on the second.
- The paired bootstrap bound reaches +0.0 and McNemar gives p = 0.219, so
  the drop is not significant on this run.

Luna, pass on v1.4.3 and fail on v1.4.4 (17): batching-7, bucket-7, config-1,
config-4, duration-2, duration-8, exporters-1, exporters-5, ledger-2,
pricing-0, pricing-1, pricing-10, pricing-2, protocol-6, ranges-4, ranges-9,
versions-1. The other way (15): cron-0, cron-6, diff-5, diff-6, duration-10,
duration-7, exporters-3, exporters-6, exporters-7, exporters-8, ledger-0,
ledger-4, pricing-9, ranges-0, versions-4. They are spread over many
templates in both directions, and the largest clusters flip both ways:
`pricing` (4 old-only, 1 new-only) and `exporters` (2 old-only, 4 new-only).
Nine of the 17 Luna pass-to-fail tasks also fail on native Luna.

**Verdict on the drops: noise.** The flips are scattered single trials, the
clusters flip both ways, and a rerun of the same model path flips as many
tasks.

### Could 1.4.3..HEAD touch claude-code or codex runs?

Not in a way that can change what the model does:

- **The antigravity adapter (#468)** adds a new provider, engine and models.
  Nothing a Claude Code or Codex agent receives (prompt, harness tool text,
  CLI arguments) is built from the provider list. Secondary routing changes
  only `antigravity/...` models. In the bench sandbox agy is on `PATH` but
  signed out, so it is unusable there.
- **Routing fix (b22f3a93f)** adds `route_before_turn` to the scheduler's
  per-agent pass. For an agent whose model and fallbacks name no
  `antigravity/...` secondary model, it returns the agent unchanged before
  reading anything. Creation's `preferred_chain` likewise leaves any
  non-secondary model as it is.
- **Startup ordering (ebdf825e8, 63d005115)** changes when antigravity is
  probed: after Claude Code and Codex are detected, not alongside them.
  Detection of those two engines is unchanged. The startup probes now also
  read the Anthropic and OpenAI sign-in once, in parallel with detection.
- **Ownership (#469)** moves the claude-code and codex evidence rule into
  `validate()` unchanged and adds a rule for antigravity. Only the error type
  of a refused record changed.
- **`provider_usage`** changes only a test's expectations. `detect.rs` adds
  `SignedOut` and `ProbeFailed` errors that only antigravity detection
  returns.

### Cost

Luna's cost point estimate is x1.16 against the x1.15 limit. Its one-sided
bound, x1.00, does not show a rise above 15 %, so the gate passes with a
warning. The same new path cost x1.06 in the full run two hours earlier, and
x1.09 more in the rerun than in the full run with no Luna-path change. Haiku
is x1.06 here and was x0.93 in the full run. Run-to-run cost variation on an
unchanged path is about the size of the warning.

## Release contents (v1.4.3..HEAD)

- #468: the antigravity engine over `agy acp`, and the Google models.
- #469: accept antigravity session-ownership evidence (every real antigravity
  task failed with "invalid engine session ownership evidence" without it).
- Fixes to #468 found by this release's `make ci` and its review:
  - 7ef588579: a signed-out agy is reported as not signed in. It was reported
    as "upgrade agy", with agy's progress line pasted in.
  - ebdf825e8: antigravity is probed after startup detection. Before, every
    daemon start on a machine with agy waited about 2 s for `agy models`
    before scheduling anything. The release process-budget test failed 5 of 5
    times on it (replay 1.7-1.9 s against its 1 s budget).
  - 63d005115: that probe stops when the daemon shuts down, so it never holds
    the store past the final sync's deadline.
  - b22f3a93f: a secondary route also gives way to a signed-in Anthropic or
    OpenAI API, and the scheduler routes an idle agent's chain again before a
    turn. An agent created or parked before an original was usable moves to
    it, the primary only until the agent holds an agy session. A fallback
    keeps agy when the API would move a subscription primary onto another
    bill.
  - 5250b1c15: a failed `agy models` is "not signed in" only when agy's words
    name a sign-in; any other failure keeps agy's own words.

## How it was run

- Final candidate: release build of `5250b1c1521550f86acf0417a0be478116c05d07`
  (tree `0e6cee9e99da31b93151a6a929f25a7f20a32ea6`), `aether 1.4.4`, binary
  SHA-256 `66d8ec94fab360628e73ea2f5af8ba8e0a69d923d93a4a5c356d5de74954e15a`.
  The commit after it on the branch adds only this directory.
- First candidate (full run): `63d005115`, binary SHA-256
  `65d49a77067ef75eda30050022279e6e71e29e0524d366a0109966489093af38`.
- Old: Aether v1.4.3 (`bf186083c880e3c9f55d6df7028dabfc658d1de9`). The
  published `aether-x86_64-unknown-linux-musl.tar.gz` was downloaded into an
  empty directory and matched the release `SHA256SUMS`, for each run. It
  reports `aether 1.4.3`, binary SHA-256
  `3a795ff2a718e8c227ac66ad6bbefd2cfe1838bfcbf77b37eac10e56ca3cc923`.
- Claude Code 2.1.332, Codex 0.172.0. Set `bench/release-set/v2` (110 tasks,
  sha256 `7f3166c96e35e2ad`), one repeat, 600 s timeout, `JOBS=6`,
  `LOAD_MAX=16`.
- Each invocation ran in a private sandbox: a fresh `HOME` holding copies of
  the Claude Code and Codex credentials and configuration (mode 600), its own
  XDG directories, `TMPDIR` and work root. The operator daemon's `AETHER_*`
  variables were unset. The sandbox and the copies were deleted when each
  invocation ended. All invocations ran inside the host CI slot.

```sh
make build
# in the sandbox (HOME, XDG_*, TMPDIR, RELEASE_BENCH_WORK_ROOT private)
# full run on 63d005115, all six arms
sh scripts/ci_slot.sh make bench-release OLD=<verified v1.4.3 aether> ALLOW_OFF_TARGET=1 \
  ARMS=all BUDGET=20 EXTRA=--run-id=rel-1-4-4
sh scripts/ci_slot.sh make bench-release OLD=<verified v1.4.3 aether> ALLOW_OFF_TARGET=1 \
  ARMS=all BUDGET=40 EXTRA=--run-id=rel-1-4-4
# new arms on 5250b1c15
sh scripts/ci_slot.sh make bench-release OLD=<verified v1.4.3 aether> ALLOW_OFF_TARGET=1 \
  ARMS=haiku/new,luna/new EXTRA=--run-id=rel-1-4-4-new2
# combined: rel-1-4-4's native and old records plus rel-1-4-4-new2's new
# records, with rel-1-4-4's config and the new binary's SHA, then
python3 evals/release_bench.py report target/release-bench/runs/rel-1-4-4-final
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

The new-arm run (2026-11-27 03:26 to 04:16 UTC, 50.4 min) ran all 220
trials at the default `BUDGET=20` and spent USD 2.70.

## make ci

`CARGO_BUILD_JOBS=6 make ci` on `5250b1c15` (tree `0e6cee9e99da`): **passed**.
Nextest ran 6901 tests: 6901 passed, 48 skipped. Two were flaky and passed on
retry. Each was then run 5 times in isolation on the same tree, and passed 5
of 5:

```sh
cargo nextest run -p aether --test cli_regressions --retries 0 \
  -E 'test(=command_polish::malformed_config_fails_auto_start_before_creating_the_store)'
cargo nextest run -p aether-daemon --test startup_probes --retries 0 \
  -E 'test(=hello_is_answered_while_engine_probes_run_and_engine_requests_wait)'
```

- `command_polish::malformed_config_fails_auto_start_before_creating_the_store`
  failed once because the daemon did not answer within its 10 s start window
  under the full suite's load. Isolated: 5 of 5 passed, 0.1 to 0.7 s.
- `startup_probes::hello_is_answered_while_engine_probes_run_and_engine_requests_wait`
  failed once with "hello took 2.2 s, of which the host stalled 680 ms".
  Isolated: 5 of 5 passed, 0.4 to 1.7 s. It also flaked under load once on
  an earlier head of this branch, and passed in isolation then too.

Both failures are timing under the parallel suite, not a code path that
fails, so neither needed a fix. The release build, e2e, the 9.8 budgets and
process measurements, the release install check and the TUI captures all
passed.

Earlier `make ci` runs on this branch failed, and their failures led to the
fixes above:

- The first, on the version bump, failed `provider_usage` and `config_errors`
  (7ef588579).
- The second failed `measured_process_budgets` 5 of 5 in isolation
  (ebdf825e8).
- The third, after merging #469, failed `shutdown_timeout` on both of its
  tries. It passed 5 of 5 in isolation, and 63d005115 removes the cause it
  exposed.
- `make ci` then passed on 63d005115 (6895 tests, 2 flaky on retry).

## Full report

The harness's `report.md` for the combined final matrix:

```text
# Release bench: INCONCLUSIVE (gate passed on an off-target set)

- Task set: `bench/release-set/v2` (110 tasks, sha256 `7f3166c96e35e2ad`) **(off target: not a release verdict)**
- Old: `3a795ff2a718e8c2`, candidate: `66d8ec94fab36062`
- Native CLIs: claude 2.1.332 (Claude Code), codex codex-cli 0.172.0
- Arms: haiku/native, haiku/old, haiku/new, luna/native, luna/old, luna/new; repeats 1; jobs 6; timeout 600s
- Trials: 660; priced spend $7.25 API-equivalent

## Arms

| Arm | Trials / planned | Infra errors | Harness failures | Pass rate [95% CI] | Check fraction | Cost / trial | Total cost | Median time |
|---|---:|---:|---:|---|---:|---:|---:|---:|
| haiku/native | 110 / 110 | 0 | 0 | 95% [91%, 99%] | 99% | $0.015 | $1.655 | 38s |
| haiku/new | 110 / 110 | 0 | 0 | 95% [90%, 98%] | 99% | $0.019 | $2.116 | 45s |
| haiku/old | 110 / 110 | 0 | 0 | 98% [95%, 100%] | 100% | $0.018 | $1.993 | 44s |
| luna/native | 110 / 110 | 0 | 0 | 39% [30%, 48%] | 80% | $0.004 | $0.398 | 45s |
| luna/new | 110 / 110 | 0 | 0 | 44% [35%, 53%] | 84% | $0.005 | $0.587 | 52s |
| luna/old | 110 / 110 | 0 | 0 | 45% [35%, 55%] | 85% | $0.005 | $0.505 | 50s |

## Release gate

### haiku: **PASS**

- **candidate vs old** (haiku/new vs haiku/old, 110 tasks, 110 pairs): pass -3.6% [-7.3%, +0.0%], check fraction -0.5% [-1.0%, +0.1%], cost x1.06 [x0.93, x1.21]; McNemar old-only 5 / new-only 1, p = 0.219
- Infra error rate: haiku/old 0.0%, haiku/new 0.0%

### luna: **PASS**

- **candidate vs old** (luna/new vs luna/old, 110 tasks, 110 pairs): pass -1.8% [-10.0%, +7.3%], check fraction -0.8% [-4.4%, +2.6%], cost x1.16 [x1.00, x1.35]; McNemar old-only 17 / new-only 15, p = 0.860
- Infra error rate: luna/old 0.0%, luna/new 0.0%
- Warning: cost point estimate x1.16 is above x1.15

## Drift reference (Aether old release vs the native CLI)

- **haiku** (haiku/old vs haiku/native, 110 tasks, 110 pairs): pass +2.7% [-1.8%, +7.3%], check fraction +0.3% [-0.4%, +1.1%], cost x1.20 [x1.07, x1.36]; McNemar old-only 2 / new-only 5, p = 0.453
- **luna** (luna/old vs luna/native, 110 tasks, 110 pairs): pass +6.4% [-3.6%, +16.4%], check fraction +4.5% [+0.2%, +9.0%], cost x1.27 [x1.11, x1.45]; McNemar old-only 12 / new-only 19, p = 0.281

A large change here between two reports means the models or native CLIs moved, not Aether.

## Pass rate by family

| Family | haiku/native | haiku/new | haiku/old | luna/native | luna/new | luna/old |
|---|---:|---:|---:|---:|---:|---:|
| algo | 100% | 100% | 100% | 75% | 88% | 75% |
| delegation | 100% | 100% | 100% | 100% | 100% | 100% |
| docspec | 98% | 95% | 100% | 21% | 38% | 38% |
| edgecase | 95% | 100% | 100% | 48% | 19% | 29% |
| longctx | 100% | 100% | 100% | 0% | 0% | 14% |
| multibug | 100% | 100% | 100% | 100% | 100% | 100% |
| port | 100% | 100% | 100% | 36% | 36% | 64% |
| refactor | 62% | 50% | 88% | 38% | 62% | 38% |
| traceback | 100% | 100% | 80% | 60% | 80% | 60% |
```

The full run's report (first candidate `63d005115`, after the resume):

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
