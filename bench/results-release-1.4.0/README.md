# Aether 1.4.0 release smoke

**Smoke verdict: no confirmed persistent candidate regression; not a statistical pass.**

Haiku: **10/10 → 10/10** tasks in both runs. Luna: **9/10 → 8/10**.
Luna confirmation: **5/9 → 5/9** on the three discordant tasks, three trials per arm.
No case repeatedly failed only on the candidate while the old version passed consistently.
The original Luna decrease and the second Haiku cost warning remain in the record.

**CI: passed.** After merging #433, the requested lifecycle/PTY checks and release build
also passed. The operator explicitly kept the pre-#433 smoke valid.

## Results

| Comparison | Old passes | New passes | Old USD/trial | New USD/trial | Old median seconds | New median seconds |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| haiku-1 | 10/10 | 10/10 | 0.008196 | 0.007742 | 27.204 | 26.409 |
| haiku-2 | 10/10 | 10/10 | 0.007040 | 0.008654 | 25.904 | 31.850 |
| luna-1 | 9/10 | 8/10 | 0.003367 | 0.002885 | 47.196 | 35.031 |

Intervals below are the harness’s paired 90% intervals (one-sided 95% gate).

- **haiku-1**: pass change +0.0 points [+0.0, +0.0]; cost ×0.945 [×0.800, ×1.124]; old-only/new-only 0/0, McNemar p=1.000.
- **haiku-2**: pass change +0.0 points [+0.0, +0.0]; cost ×1.229 [×1.133, ×1.322]; old-only/new-only 0/0, McNemar p=1.000.
- **luna-1**: pass change -10.0 points [-40.0, +20.0]; cost ×0.857 [×0.725, ×1.025]; old-only/new-only 2/1, McNemar p=1.000.

All 60 planned smoke trials completed, with no infrastructure errors, harness failures,
or unpriced trials. Costs are API-equivalent. Each invocation exited 1 with
`INCONCLUSIVE`: the harness requires 20 distinct paired tasks, while this requested
smoke uses 10. The frozen set is also off its calibration target. Neither limitation
was removed or relabeled as a statistical pass.

The second Haiku run’s cost point estimate exceeds the 15% margin, but its interval
crosses that margin; the first run was cheaper. Luna’s original −10-point accuracy
estimate has a wide interval crossing zero. Confirmation results below do not erase
either observation. These samples cannot exclude smaller or task-specific regressions.

## Luna confirmations

Each originally discordant case ran three times per arm, strictly old/new alternating.
These selected-case trials are reported separately, not pooled into the original smoke.

| Task | Original old checks | Original new checks | Confirm old checks (three trials) | Confirm new checks (three trials) |
| --- | ---: | ---: | --- | --- |
| batching-7 | 9/9 | 2/9 | 9/9, 3/9, 9/9 | 9/9, 9/9, 9/9 |
| bucket-0 | 8/8 | 7/8 | 7/8, 7/8, 8/8 | 7/8, 7/8, 8/8 |
| bucket-10 | 3/8 | 8/8 | 7/8, 8/8, 8/8 | 7/8, 8/8, 7/8 |

Selected-case confirmation cost ratio: ×1.162 [×1.063, ×1.260].
Its point estimate also exceeds the 15% margin, while the interval crosses it.
These three selected cases are not an estimate over the frozen task population.

The candidate passed all three `batching-7` confirmations; old passed two.
Both versions had the same 1/3 pattern on `bucket-0`. On `bucket-10`, old passed 2/3
and new 1/3, reversing the original candidate-only success. Thus the operator’s
specified persistent one-sided failure condition was not observed. This is limited
smoke evidence, not proof that the versions are equivalent.

## Scope, ordering, and load

The first ten tasks in the frozen v2 set’s sorted order were selected before any results:
`batching-0`, `batching-5`, `batching-7`, `billing-1`, `billing-7`, `billing-8`,
`billing-9`, `bucket-0`, `bucket-1`, `bucket-10`. No prompts, hidden checks, solver
settings, price table, or regression thresholds were tuned. Each trial ran alone
in a private workspace with its own daemon/config/data/socket. Engine credentials
were copied with mode 600 and removed after the run.

The standard schedule kept task pairs adjacent and alternated which arm went first.
Confirmation trials strictly alternated old then new. Initial smoke waited for load5
<12, then started under the operator’s amended per-trial load5 <14 rule. Confirmations
used the later load5 <20 rule. The wrapper recorded the 5-minute load immediately
before each trial and preserved the memory and spend guards.

The shared host repeatedly delayed admission. Wall times are descriptive: model
variability and differing host load prevent attributing these timing differences to
the release. Load-driven timing movement is not treated as a regression.

An attempted extension to meet the harness’s 20-pair minimum completed three extra
passing trials (`config-6` old/new, `cron-0` new). The operator then explicitly
cancelled that extension and defined the release smoke as the original ten tasks.
Those extra trials are retained as abandoned evidence and excluded from all results above.

### Load5 at every trial start

Each row contains both independently measured starts; repeats are zero-indexed.

#### haiku-1

| Task | Repeat | Old load5 | New load5 |
| --- | ---: | ---: | ---: |
| batching-0 | 0 | 12.731445 | 13.123047 |
| batching-5 | 0 | 12.319336 | 11.949219 |
| batching-7 | 0 | 13.676270 | 13.179199 |
| billing-1 | 0 | 13.928711 | 13.948242 |
| billing-7 | 0 | 13.991211 | 13.988281 |
| billing-8 | 0 | 13.567383 | 12.766602 |
| billing-9 | 0 | 11.919922 | 11.951172 |
| bucket-0 | 0 | 13.829590 | 13.559570 |
| bucket-1 | 0 | 12.726074 | 13.075195 |
| bucket-10 | 0 | 12.353027 | 12.149414 |

#### haiku-2

| Task | Repeat | Old load5 | New load5 |
| --- | ---: | ---: | ---: |
| batching-0 | 0 | 12.945801 | 12.387695 |
| batching-5 | 0 | 11.951172 | 12.832031 |
| batching-7 | 0 | 11.817871 | 12.150391 |
| billing-1 | 0 | 13.969238 | 13.371094 |
| billing-7 | 0 | 11.736328 | 12.542969 |
| billing-8 | 0 | 13.946777 | 13.914551 |
| billing-9 | 0 | 13.941406 | 13.830566 |
| bucket-0 | 0 | 12.401367 | 12.199219 |
| bucket-1 | 0 | 11.823730 | 12.386719 |
| bucket-10 | 0 | 13.429199 | 13.782715 |

#### luna-1

| Task | Repeat | Old load5 | New load5 |
| --- | ---: | ---: | ---: |
| batching-0 | 0 | 13.099121 | 13.186035 |
| batching-5 | 0 | 13.933594 | 13.968262 |
| batching-7 | 0 | 12.829590 | 12.960938 |
| billing-1 | 0 | 13.775391 | 13.087891 |
| billing-7 | 0 | 12.332520 | 12.389160 |
| billing-8 | 0 | 13.935059 | 13.823730 |
| billing-9 | 0 | 12.831543 | 13.098633 |
| bucket-0 | 0 | 13.764160 | 13.620117 |
| bucket-1 | 0 | 13.743164 | 13.487305 |
| bucket-10 | 0 | 13.152344 | 13.121582 |

#### luna-confirm

| Task | Repeat | Old load5 | New load5 |
| --- | ---: | ---: | ---: |
| batching-7 | 0 | 13.714355 | 14.485352 |
| batching-7 | 1 | 14.209473 | 13.956543 |
| batching-7 | 2 | 13.896484 | 14.888672 |
| bucket-0 | 0 | 14.465820 | 13.964844 |
| bucket-0 | 1 | 13.284668 | 12.818359 |
| bucket-0 | 2 | 12.386230 | 11.509277 |
| bucket-10 | 0 | 11.184082 | 11.415039 |
| bucket-10 | 1 | 11.249512 | 10.923340 |
| bucket-10 | 2 | 10.682129 | 11.162109 |

## Reproduction and provenance

- Smoke runtime commit: `e57ab2569b839813455d132ca31ca9b561817803`.
- Full-CI tree: `f81b902bb37a4c741ebf7c3eb81f8e7b4c06bc10`.
- Published Aether v1.3.1 source: `3e3ef6e68c53b9b5c77400aa411ff13f0519733f`.
- The pre-existing local `v1.3.1` tag names an unrelated Archon release; it was not changed.
- The published Linux x86_64 musl archive was verified against release `SHA256SUMS`:
  `8138d11f31e435547f00b7808c7e9a7458bacf6b17903848baac54c2ccf1a6e9`.
- Old binary SHA-256: `ad05f728843ef71e69e8f4658be9fd7c7ec19f42f7f3227366e88993dca67386`.
- Smoke candidate SHA-256: `ec32f0163ac8fd07b279e989bd5c54203725658ef635c01d54765ab8405f8dcf`.
- Post-#433 code commit: `69454e84d33b65118a14d1614be766a2e82ad0c5`.
- Post-#433 code tree: `e9019e5c6eed9f2abe16432fb2859820ccc23b6d`.
- Final release binary SHA-256: `8b4c717ef1392ed0afd845a24d077b2795b7ec10654365bfab4e10ee61463592`.
- #433 was merged from `origin/next` at `ef490ab35`; no installed daemon was replaced.
- Raw logs, result JSONL, configurations, load readings, and the load-measuring wrapper
  are retained in the release-check artifact. Private sandboxes and copied credentials
  were removed. No tag, push, publication, or pull request was performed in this stage.

Harness arguments for each of the two Haiku runs and the Luna run:

```sh
python3 .archon/smoke-entry.py run --old "$OLD" --new "$NEW" \
  --arms "$TIER/old,$TIER/new" --jobs 1 --load-max 14 --budget 20 \
  --allow-off-target --limit 10 --run-id "$RUN" --work-root "$PRIVATE_WORK"
```

The wrapper uses the checked-in harness with 5-minute admission rather than its
default 1-minute gate. For confirmation it selected only the three discordant cases,
used `--limit 3 --repeats 3 --load-max 20`, and imposed strict old/new alternation.

## Verification

`CARGO_BUILD_JOBS=3 make ci` passed before #433. It started at load5 11.765137
and waited for the host CI slot. The item-specific Cargo cache was used throughout,
with development/test debug information and incremental compilation disabled for
disk headroom, and three nextest threads.

| Suite | Result |
| --- | --- |
| Workspace nextest | 6,698 passed, 47 skipped; no retry |
| Rust documentation tests | Passed; no runnable cases |
| Python suites, including Python 3.11 repeat | 461 tests; one skip |
| TypeScript client | 12 passed |
| Remote-web unit tests | 204 passed, 49 skipped |
| Release timing/budget/process tests | 14 passed |
| Formatting, Clippy, shell, notices, links, generated-client drift | Passed |
| Mock e2e, latest/pinned release install, terminal captures | Passed |

Limits: Chromium was unavailable, so browser/live-web checks did not run; the container
lane had no configured image. The installer’s dash-specific test was skipped.
Idle CPU was measured but remains a documented unenforced budget; periodic housekeeping
still violates the no-timers goal. These are not newly passing claims.

The operator then required #433 in this release and explicitly retained the smoke.
The following affected-test run passed **703 tests** with no retries,
starting at load5 **12.540039**; the release build passed starting at
load5 **15.667480**. Both used four build jobs, per the amendment.

```sh
CARGO_BUILD_JOBS=4 cargo nextest run --workspace --build-jobs 4 -j 4 \
  --no-fail-fast --retries 0 \
  -E 'package(aether-spawn) | package(aether-engine) | test(/spawn|detach|lifeline|pty|signal|session|terminal/)'
CARGO_BUILD_JOBS=4 cargo build --release -j 4
```

The broader full suite was not repeated after #433, as directed; the affected filters
include all engine/spawn tests and daemon/client PTY, detachment, lifeline, session,
terminal, and signal cases. Final notes/report-only changes received notice and link
checks, formatting verification, and release-note extraction.

## What 1.4.0 contains

- [#430](https://github.com/mengsig/aether/pull/430): bounded `delegation_policy`.
- [#431](https://github.com/mengsig/aether/pull/431): approved `references` survive delegation and compaction.
- [#432](https://github.com/mengsig/aether/pull/432): ask-before-guessing guidance and sequence-keyed questions.
- [#427](https://github.com/mengsig/aether/pull/427), [#428](https://github.com/mengsig/aether/pull/428): remote Attention dismissal and phone-width error positioning.
- [#433](https://github.com/mengsig/aether/pull/433): daemon/engine terminal detachment and session-aware watchdog cleanup.
- Workspace, Python client, and TypeScript client versions are 1.4.0.

Follow-ups remain: standalone `CreateTask` references, cross-provider tier comparison,
and grandchild delegation policy. This report adds no claim of full benchmark coverage.
