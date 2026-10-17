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

Selecting on measured rates favors tasks that looked mid-range by chance, so a
fresh run of the frozen set usually shows rates a little nearer the extremes
than `calibration.md` reports. Recalibrate when the native pass rates in
release runs drift well away from the targets, for example after a model
update.

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
