# Aether 1.4.2 release smoke

**Smoke verdict: no decrease against v1.4.1; not a statistical pass.**

Haiku: **9/10 → 10/10** and **10/10 → 10/10**. Luna: **5/10 → 9/10**.
No task passed on v1.4.1 and failed on the candidate in any run, so there was
no failing case to confirm with another run. Luna's cost point estimate
(×1.26) is above the 15 % margin, but its interval crosses it, and the
candidate solved four more tasks than v1.4.1 did.

**CI: passed** (`CARGO_BUILD_JOBS=3 make ci` in the host CI slot; see below).

## Results

| Comparison | Old passes | New passes | Old USD/trial | New USD/trial | Old median seconds | New median seconds |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| haiku-1 | 9/10 | 10/10 | 0.0091 | 0.0090 | 37 | 38 |
| haiku-2 | 10/10 | 10/10 | 0.0097 | 0.0092 | 41 | 34 |
| luna-1 | 5/10 | 9/10 | 0.0035 | 0.0044 | 33 | 40 |

Intervals are the harness's paired 90 % intervals (one-sided 95 % gate).

- **haiku-1**: pass +10.0 points [+0.0, +30.0]; cost ×1.00 [×0.92, ×1.07];
  old-only/new-only 0/1, McNemar p=1.000. The old miss was `bucket-1` (7/8 checks).
- **haiku-2**: pass +0.0 points [+0.0, +0.0]; cost ×0.94 [×0.86, ×1.04];
  old-only/new-only 0/0, McNemar p=1.000.
- **luna-1**: pass +40.0 points [+19.5, +70.0]; cost ×1.26 [×0.83, ×1.86];
  old-only/new-only 0/4, McNemar p=0.125. Both failed `batching-7` (2/9 checks);
  only v1.4.1 failed `bucket-0`, `bucket-1`, `batching-5` and `bucket-10`.

All 60 trials completed with no infrastructure errors, harness failures or
unpriced trials. Costs are API-equivalent. Each invocation exited 1 with
`INCONCLUSIVE`: the harness needs 20 distinct paired tasks and this smoke uses
10, and the v2 set is off its calibration target. Neither limit was removed.

The Luna cost rise comes with the candidate's extra solves: two of them ran
long (`batching-5` 300 s, and `batching-0`, which both passed, 391 s against
56 s), while v1.4.1 stopped early on the tasks it failed. On the five tasks both arms passed, the
candidate cost $0.0195 against $0.0239 for v1.4.1 (×0.82). One Luna run cannot
separate model variance from the release; in 1.4.0's Luna
confirmations one binary passed and failed the same task across repeats. These samples cannot exclude
smaller or task-specific regressions.

## Scope, ordering and load

The release set v2's first ten tasks in sorted order, as in 1.4.0:
`batching-0`, `batching-5`, `batching-7`, `billing-1`, `billing-7`,
`billing-8`, `billing-9`, `bucket-0`, `bucket-1`, `bucket-10`. No prompts,
checks, solver settings, price table or thresholds were changed.

Each run had its own private sandbox: a fresh `HOME` holding copies of the
Claude Code and Codex credentials (mode 600), its own work root, and per-trial
daemons, configs, data directories and sockets from the harness. The sandbox
and the copied credentials were deleted when each run ended. Runs went one at
a time, with one trial at a time; the harness alternated which arm of a pair
went first. Each trial was admitted only while the 5-minute load was below 14
(a wrapper feeds the harness's gate the 5-minute value instead of the
1-minute one). The admission loads were 9.5–13.6 (haiku-1), 10.7–14.0
(haiku-2) and 7.9–14.0 (luna-1). `make ci` overlapped the second Haiku run
and the Luna run, so wall times are descriptive only.

## Reproduction and provenance

- Candidate: release build of `885977242ad78eb0afb22ed19ad73663b883adfe`
  (tree `1b629b5eace328a70eb685d7cabbed182861ff57`), `aether 1.4.2`,
  binary SHA-256 `63b35352a23ebbc7a84241e2b963fc3157c2db5248d4d3e10c0b05f6be731eb8`.
  Later commits on the release branch add only this report.
- Old: Aether v1.4.1 (`923be7b0adcaeb267557956b917353aa717ea610`). The published
  `aether-x86_64-unknown-linux-musl.tar.gz` matched the release `SHA256SUMS`
  (`5695bb5cd640957bef906d078ebe69d6d0fcde0538c4349c3bb4885a3f21176c`);
  binary SHA-256 `88ed802b54ae2a91ac67cb960b100d441e9376d53ef973e6fb6785005ac86de9`.
  The local `v1.4.1` tag names an unrelated Archon release; it was not changed.
- Claude Code 2.1.325, Codex 0.169.0.

Each run, with `$TIER` `haiku` (twice) or `luna`:

```sh
python3 smoke_entry.py "$REPO" "$LOAD_LOG" run --old "$OLD" --new "$NEW" \
  --arms "$TIER/old,$TIER/new" --jobs 1 --load-max 14 --budget 20 \
  --allow-off-target --limit 10 --run-id "$RUN" --work-root "$PRIVATE_WORK"
```

`smoke_entry.py` runs `evals/release_bench.py` unchanged, with the 5-minute
load in place of the 1-minute load and a log of the load at each admission.
Run IDs: `r142-haiku-1`, `r142-haiku-2`, `r142-luna-1`.

## Verification

`CARGO_BUILD_JOBS=3 make ci` passed on `885977242` (tree `1b629b5e`), the
smoke candidate's commit. It started at load5 13.95 and waited about 70
minutes in the host CI slot queue. The item's own Cargo cache was used.

| Suite | Result |
| --- | --- |
| Workspace nextest | 6,733 passed, 47 skipped; 1 flaky (below) |
| Rust documentation tests | Passed; no runnable cases |
| Python, TypeScript and remote-web unit suites | Passed |
| Formatting, Clippy, shell, notices, links, generated-client drift | Passed |
| Mock end-to-end, release timing and budgets, latest/pinned install check, TUI captures | Passed |

`aether::daemon_session direct_detached_daemon_leads_a_session_without_a_terminal`
missed its five-second listen deadline once and passed on nextest's retry,
while the host also ran the Luna smoke. Five isolated reruns afterwards
passed 5/5 (0.37–1.19 s), so it was left as a load-sensitive timing test.

Not run, as in 1.4.0: the Chromium browser checks (no Chromium installed) and
the container backend lane (no image configured).

## What 1.4.2 contains

- [#443](https://github.com/mengsig/aether/pull/443): per-agent compaction threshold (`config.compaction_tokens`).
- [#444](https://github.com/mengsig/aether/pull/444): the model catalog reports superseded and retired models
  and the efforts each route sends distinctly; turns record the requested and effective effort.
- Workspace, Python client and TypeScript client versions are 1.4.2.

Follow-ups: per-model effort data for engine routes, retirement guidance that
leaves out disabled providers, and delegation refusals that name a superseded
model's former rank.
