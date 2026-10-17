# Release bench

`make bench-release` compares the last release of Aether with a release
candidate, and both with the plain native CLIs, on a frozen set of short coding
tasks. It is meant to run before every release: it costs about USD 15
API-equivalent at most, runs trials in parallel, and prints a Markdown report
ending in a PASS or FAIL verdict.

```sh
make build                                     # the candidate: target/release/aether
make bench-release OLD=path/to/aether-1.1.0    # exit 0 only on PASS
```

The run needs the `claude` and `codex` CLIs logged in, `git`, and Python 3.11
or newer. Nothing is installed or written outside `target/release-bench/` and
the work root (below).

## What runs

Every task in the set runs once (by default) on six arms:

| Arm | Harness | Model, effort |
| --- | --- | --- |
| `haiku/native` | Claude Code (`claude -p`) | Claude Haiku 5.5, low |
| `haiku/old`, `haiku/new` | one Aether agent on the `claude-code` provider, old or candidate binary | Claude Haiku 5.5, low |
| `luna/native` | Codex (`codex exec`) | GPT-6 Luna, low |
| `luna/old`, `luna/new` | one Aether agent on the `codex` provider, old or candidate binary | GPT-6 Luna, low |

The native arms do not depend on Aether. They are the drift reference: when the
native pass rate moves between two runs, the provider or CLI changed, not
Aether. Pass `ARMS=haiku`, `ARMS=luna` or explicit keys such as
`ARMS=haiku/old,haiku/new` to run fewer arms.

Each trial gets a fresh git workspace with the task's files. The workspace
sits under the work root, outside the repository, so no solver can read the
generators and their reference solutions. Each trial also gets its own TMPDIR,
XDG cache and state, and, for Aether arms, its own daemon, config, data
directory and socket. Trials therefore run side by side (`JOBS=6` by default).
A trial starts only while the 1-minute load is at most `LOAD_MAX` (16) and at
least 2 GB of memory is free.

Caps per trial: a 600 s timeout; USD 0.50 for Claude arms (`--max-budget-usd`
natively, `cost_budget` through Aether); 4 M tokens for Luna through Aether
(Codex reports no cost to Aether, and `codex exec` takes no cap). Before
starting a trial, the run reserves that trial's worst case against `BUDGET`
(USD 15). It stops starting trials when the reservation would not fit, and the
report says so.

After the solver stops, hidden checks (a `unittest` module the solver never
sees) run against the workspace in a separate, isolated Python process. A trial
**passes** when every hidden check passes. The fraction of checks passed is
reported too.

## Reading the report

`target/release-bench/runs/<run-id>/report.md` (also printed) holds:

- **Arms:** pass rate, mean check fraction, mean API-equivalent cost and time
  per arm.
- **Gate, per model tier:** the candidate (`new`) against the last release
  (`old`), on the same tasks and repeats.
  - The pass-rate difference with a one-sided 95 % bound, from a paired
    bootstrap over tasks. Repeats of a task are averaged first, so the task is
    the unit of resampling.
  - An exact McNemar test on the discordant pass/fail pairs.
  - The cost ratio new/old, with its bound.
- **Drift:** `old` against `native`, for reference only.
- **Per family:** pass rates by task family.

The verdict for a tier is:

- **FAIL** when, at 95 % one-sided confidence, the candidate's pass rate is more
  than 5 points below the old release's, or its cost is more than 15 % above it.
- **INCONCLUSIVE** when fewer than 20 tasks were paired, or more than 10 % of
  the tier's old/new trials hit an infrastructure error. An infrastructure
  error is a harness crash, no model output at all, or a cleanup failure. These
  trials are excluded, not counted as failures.
- **PASS** otherwise.

The overall verdict is FAIL when any tier fails, PASS when every tier passes,
and INCONCLUSIVE otherwise (also when no tier ran both an old and a candidate
arm). The make target exits 0 only on PASS. A point estimate beyond a margin that is not
significant is listed as a warning.

`EXTRA=--early-stop` looks at the gate when half and when three quarters of
the tasks are paired. It stops only for a decisive FAIL, never for a pass. Each
of the three looks (the final gate included) then uses a Bonferroni share of
the 5 % error rate.

Re-render a report, for example after changing margins, with
`python3 evals/release_bench.py report target/release-bench/runs/<run-id>`.
An interrupted run resumes when started again with the same `--run-id`
(`EXTRA=--run-id=...`). Its configuration must match, or the run refuses to
mix the two.

## Make variables

| Variable | Default | Meaning |
| --- | --- | --- |
| `OLD` | required | aether binary of the last release |
| `NEW` | `target/release/aether` | aether binary of the candidate |
| `SET` | `bench/release-set/v1` | frozen task set |
| `ARMS` | `all` | `all`, `haiku`, `luna`, or comma-separated arm keys |
| `JOBS` | 6 | trials at once |
| `LOAD_MAX` | 16 | start no trial above this 1-minute load |
| `BUDGET` | 15 | USD API-equivalent; stop starting trials beyond it |
| `REPEATS` | 1 | runs of every (task, arm) |
| `ARCHON_BIN` | unset | adds an Archon arm per tier, through the adapter below |
| `EXTRA` | unset | more `release_bench.py run` options, such as `--limit 10` or `--early-stop` |

Set `RELEASE_BENCH_WORK_ROOT` to move the workspaces. The default is
`$TMPDIR/aether-release-bench`.

## The task set

`bench/release-set/vN/` is frozen. `tasks.jsonl` holds every byte a trial
sees, plus its hidden checks and reference solution. `manifest.json` holds a
SHA-256 per task and one over the set, and a run refuses a set whose bytes
changed. `calibration.md` records how the set was chosen.

Tasks come from parametrized templates in `evals/release_bench_families/`. A
(template, seed) pair always yields the same task, while names, rules and
defects vary by seed. Each task is 5–15 tool calls of work for a capable
model:

| Family | Template | The work |
| --- | --- | --- |
| docspec | `config`, `cron`, `duration`, `query`, `versions` | implement a format or query language whose interacting rules are spelled out in a long doc |
| edgecase | `bucket`, `ranges`, `slug` | handle the cases the visible tests miss |
| multibug | `billing` | audit a package against its docs; the issue reports only some of the defects |
| traceback | `ledger` | fix a crash from a log and traceback, then the silent defects the same input exposes |
| port | `pricing` | finish an exact Python port of a legacy JavaScript module (coercion, rounding and key-order quirks) |
| algo | `diff`, `batching` | replace a plausible heuristic with the exact algorithm the doc defines (a minimal line diff with fixed tie-breaks; the cheapest batch plan under tie rules) |

`python3 evals/release_bench_generate.py --count N` generates candidates and
keeps only fair ones:

- the starter fails at least one hidden check;
- the reference solution passes them all;
- grading the same files twice gives the same result check by check.

Hidden checks often compare the workspace's code with the reference on
hundreds of generated inputs. The inputs always stay inside the documented
rules.

### Calibrating a new set

```sh
make bench-calibrate FREEZE=bench/release-set/v2   # NEW's price table; CAL_BUDGET=25, CAL_REPEATS=3
```

This runs `COUNT` (200) verified candidates `CAL_REPEATS` times on the two
native arms. A task is eligible when it never hit an infrastructure error and
is not saturated: a task both models solve every time, or neither ever solves,
cannot show a change on either tier, so it only adds cost. The run then trims
the eligible tasks (80–120, no template above 20 %) so the mean native Haiku
pass rate lands near 50 % and Luna's near 30 %, preferring tasks whose own
Haiku rate is nearest 50 %, and freezes the set.

At low effort both models are nearly deterministic on a given task: most tasks
sit at 0 % or 100 % for a model, and few in between. The targets therefore
apply to the set's mean, which gives a regression room to show on either side.

Selecting on measured rates favors tasks whose measured rate erred toward the
target by chance, so a fresh run of the frozen set usually shows rates further
from the targets than `calibration.md` reports. Recalibrate when the native pass rates in
release runs drift well away from the targets, for example after a model
update.

### Set v1

Set v1 was calibrated on 2026-10-17 from 143 candidates (11 seeds of each of
the 13 templates). Each ran twice on both native arms: 572 trials, USD 6.33
API-equivalent, 61 minutes at 8 trials at once, no infrastructure error.

- **Luna** passed 149 of 286 trials. The chosen 92 tasks have a mean native
  Luna pass rate of 30 %, on target.
- **Haiku** passed 281 of 286 trials. Its only misses were one run each of
  three `query` tasks and two `ranges` tasks. The chosen set's mean native
  Haiku pass rate is 97 %, far above the 50 % target.

No template brought native Haiku near 50 %. That held across long interacting
specs, hidden edge cases, multi-defect audits, tracebacks, an exact JavaScript
port and exact algorithms. In pilots, most Haiku misses traced back to an
ambiguous doc or a check the doc did not cover. Those were fixed rather than
kept. The one genuine pilot miss, a config rule, did not recur in calibration.
This matches the earlier [harness measurements](benchmarks.md), where Haiku 5.5
passed over 99 % of hidden checks on much larger tasks. Fair tasks of 5–15 tool calls with
deterministic checks do not stretch Haiku 5.5 at low effort.

**v1 therefore misses the Haiku calibration target.** Its Haiku tier still
detects a candidate that loses tasks the last release solved, which is what a
release gate needs. It has almost no room to show an improvement, and few tasks
on which old and new can disagree. The Luna tier has room on both sides.

Dropping the tasks native Haiku always passes leaves 5 of 143 candidates, too
few for a set. Outcomes are nearly deterministic per task. A Haiku rate near
50 % therefore needs about half the set to be tasks Haiku consistently fails yet
a reference solves, such as competition-grade algorithm problems with
documented input sizes and time limits. That is a new task family, left as a
follow-up.

### A/A check of v1

`make bench-release` with Aether 1.1.0 as both `OLD` and `NEW` ran all 552
trials (92 tasks × 6 arms, `JOBS=6`) in 101 minutes for USD 6.90
API-equivalent, with no infrastructure error. Its verdict was **PASS** on both
tiers.

| Arm | Pass rate [95 % CI] | Check fraction | Cost / trial | Median time |
| --- | --- | ---: | ---: | ---: |
| `haiku/native` | 97 % [92 %, 100 %] | 99 % | USD 0.018 | 49 s |
| `haiku/old` | 97 % [92 %, 100 %] | 99 % | USD 0.018 | 45 s |
| `haiku/new` | 97 % [92 %, 100 %] | 99 % | USD 0.022 | 48 s |
| `luna/native` | 45 % [35 %, 54 %] | 80 % | USD 0.004 | 25 s |
| `luna/old` | 47 % [37 %, 58 %] | 84 % | USD 0.007 | 36 s |
| `luna/new` | 49 % [39 %, 60 %] | 85 % | USD 0.007 | 35 s |

- **Haiku gate:** pass rate +0.0 points [−3.3, +3.3]; McNemar 2 against 2,
  p = 1.000. The cost ratio was ×1.22 [×1.05, ×1.41], so the report printed a
  warning but no FAIL. Most of that gap comes from two `pricing` trials, whose
  Haiku cost ranges from USD 0.03 to 0.21. Cost per trial is heavy-tailed, so
  read a single cost warning with care.
- **Luna gate:** pass rate +2.2 points [−8.7, +12.0]; McNemar 16 against 18,
  p = 0.864; cost ×1.03 [×0.87, ×1.22].

Luna passed about 47 % of v1 here, against 30 % in calibration. The trim toward
30 % kept tasks that looked hard for Luna by chance (see above), and Luna's
outcomes vary more between runs than Haiku's.

The run took longer than an hour at `JOBS=6`. Other work on the host held it at
the load gate for about 3 minutes in all, so `JOBS=8` or more shortens it when
memory allows. Calibration ran at `JOBS=8` without trouble. The full report is
[release-bench-2026-10-18-aa.md](release-bench-2026-10-18-aa.md).

## The Archon arm (adapter contract)

`ARCHON_BIN=/path/to/adapter` adds `haiku/archon` and `luna/archon`. The
adapter is any executable. It runs with the task workspace as its working
directory and these variables set:

| Variable | Value |
| --- | --- |
| `BENCH_PROMPT_FILE` | file holding the exact task text every arm gets |
| `BENCH_WORKSPACE` | the workspace (also the working directory) |
| `BENCH_MODEL` | `claude-code/claude-haiku-5-5` or `codex/gpt-6-luna` |
| `BENCH_EFFORT` | `low` |
| `BENCH_USAGE_FILE` | where the adapter may write `{"api_equivalent_usd": number, "total_tokens": number}` |

The adapter edits the workspace and exits 0 on success. Its stdout and stderr
are kept in the trial directory. The bench grades the workspace as for every
other arm and kills the adapter's process group at the timeout. Without a
usage file the trial counts as unpriced, and the budget assumes its worst case.

## Tests

```sh
python3 -m unittest discover -s evals -p 'test_release_bench*.py'
```

These tests cover the statistics (McNemar, bootstrap, the gate, interim
looks), the scheduling, budget and resume logic of the pool, selection,
frozen-set integrity, arm caps, trial records and one generated task per
template. None of them calls a model.
