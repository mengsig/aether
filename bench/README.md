# Bench

The frozen task sets and the recorded results of the release bench
(`evals/release_bench.py`, [docs/release-bench.md](../docs/release-bench.md)).

| Directory | What it holds |
| --- | --- |
| `release-set/v1`, `release-set/v2` | the earlier sets; v2 stays the default `SET` |
| `release-set/v3` | the hard set, calibrated on plain Aether (`new`), not on the native CLIs |
| `results-release-*` | reports of past release runs |
| `memory-set` | the memory bench's set |

Each set's `tasks.jsonl` holds its hidden checks and reference solutions. Keep
it away from anyone working on Aether or Archon. Quote a run's
`failure-digest.md`, which names no task, rather than its `report.md`.

## One run gates both releases

One `make bench-release` runs every arm on the same tasks at once: the native
CLIs, Aether old and new, and Archon old and new, on Haiku 5.5 and Luna. The
report ends in an Aether verdict and an Archon verdict.

```sh
make build                                     # the Aether candidate: target/release/aether
make bench-release SET=bench/release-set/v3 \
  OLD=path/to/aether-<last release> NEW=target/release/aether \
  ARCHON_OLD=path/to/archon-<last release> ARCHON_NEW=path/to/archon-<candidate> BUDGET=100
```

That runs ten arms per task: `<model>/native`, `/old`, `/new`, `/archon-old`
and `/archon-new` for both models. `archon-old` runs on the `OLD` Aether,
`archon-new` on `NEW`. The Archon arms run Archon's `adaptive` pipeline with
`standard` review, and the reviewer runs on the arm's model. `direct` hands
risky work, such as a migration, back to the orchestrator rather than doing
it, and with no orchestrator to take it back that would count as a failure.
`ARCHON_PIPELINE` and `ARCHON_REVIEW` override that, and the report records
the values. `ARCHON_REVIEW=none` is for ablations only; it is not a release
run. `ARCHON_BIN` still works as another name for `ARCHON_NEW`.

To gate only one product:

```sh
make bench-release SET=bench/release-set/v3 OLD=... NEW=...                            # Aether only
make bench-release SET=bench/release-set/v3 OLD=... NEW=... ARCHON_OLD=... ARCHON_NEW=... \
  ARMS=haiku/new,haiku/archon-old,haiku/archon-new,luna/new,luna/archon-old,luna/archon-new  # Archon only
```

An Archon release with an unchanged Aether passes the same Aether binary as
`OLD` and `NEW`.

### Parallel trials

Trials run at once up to `JOBS`, and none starts above `LOAD_MAX` (the
1-minute load) or below 2 GB of free memory. The defaults come from the host:
`JOBS` is the core count minus two, at most one per 1.2 GB of memory, between
2 and 10; `LOAD_MAX` is 1.25 times the core count. A 12-core, 15 GB host gets
10 and 15. Each block of trials runs one task's arms back to back, and the old
and new arm of each pair alternate in which starts first. So host load falls on
both sides of a comparison alike. The report's "Load and wall time" section
gives the 1-minute load each arm started under, and the speed-up of the run
over the same trials run one at a time. On 9 Haiku trials of v2, one job took
397 s and nine jobs took 58 s: a 6.8x speed-up.

`BUDGET` caps spend, and it also limits parallelism. Each running trial
reserves its worst case against `BUDGET`: up to USD 9 for a Haiku Archon trial
and USD 3 to 6 for other Haiku trials. With the default of 20, a ten-arm run
gets about five Haiku trials at once, whatever `JOBS` says. `BUDGET=100` lets
10 workers run. The report's "waited ... for budget headroom" line shows when
the budget held workers back.

### Reading the verdicts

- **Aether release gate**: `new` against `old` per model, on accuracy and
  cost.
- **Archon release gate**: `archon-new` against `archon-old` the same way,
  plus the parity rule, which is: on every model, `archon-new` passes at least
  as often as `new` on the same (task, repeat) trials. The report prints
  `PARITY MET`, or `PARITY MISSED` with the tasks `new` passed more often.
  Archon may cost more and take longer.
- **Failure digest**: per arm, why trials did not pass (timeout, spend
  allowance, review rounds exhausted, harness error, wrong result by share of
  checks passed, and so on). It is also written to `failure-digest.md` with no
  task names, so it can be passed on.
- `verdict.json` holds both products' verdicts and the parity per model for
  scripts.

A FAIL on either gate, or a parity miss, stops that product's release.

## Calibrating a hard set

The calibration of `release-set/v3` measured plain Aether (`new`) on the
candidate tasks, because the set's target was a 30–70 % pass rate for `new`
on both models. This is the command that froze it:

```sh
python3 evals/release_bench.py calibrate --count 24 --templates bundle \
  --price-binary ~/.local/bin/aether --arms haiku/new,luna/new --select-harness new --repeats 2 \
  --min-repeats 2 --haiku-band 0.0,1.0 --min-size 20 --max-size 24 --haiku-target 0.5 \
  --haiku-tolerance 0.2 --luna-target 0.5 --luna-band 0.3,0.7 --max-template-share 1 \
  --release-arms shared --release-jobs 10 --max-release-minutes 90 --jobs 8 --budget 60 \
  --timeout 900 --freeze bench/release-set/v3
```

Every v3 task is a `bundle`: three different hard tasks in one repository,
each with its own package and doc, and all to be fixed. Single hard tasks
passed about 90 % on Haiku in a pilot, too easy to show a gain. Bundling adds
breadth, not trick checks: every part keeps its own verified checks. So
`--max-template-share 1` lifts the 20 % rule, which cannot apply to a set of
one template; the parts mix all six hard templates.

`--release-arms shared` sizes the set for the ten-arm run. `--arms` also
takes `native` and `archon-new` arms, with `--archon-new`.
`release-set/v3/calibration.md` records the exact run.
