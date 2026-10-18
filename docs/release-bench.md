# Release bench

`make bench-release` compares the last release of Aether with a release
candidate, and both with the plain native CLIs, on a frozen set of short coding
tasks. It runs trials in parallel, stops starting them at `BUDGET` (USD 20
API-equivalent), and prints a Markdown report that ends in a PASS, FAIL or
INCONCLUSIVE verdict.

```sh
make build                                                        # the candidate: target/release/aether
make bench-release OLD=path/to/aether-1.1.0 ALLOW_OFF_TARGET=1    # v2 is off target (below)
```

The run needs the `claude` and `codex` CLIs logged in, `git`, and Python 3.11
or newer. Nothing is installed or written outside `target/release-bench/` and
the work root (below).

## Status

The release bench is a release-time regression check, not a calibrated gate.

- **Only before a release.** Run `make bench-release` once per release, on
  the release candidate. Never run it per item, per pull request or in CI;
  batch work into releases instead. CI (`scripts/ci.sh`) runs only the bench's
  unit tests, which call no model.
- **The set is off target.** On the default set, v2, native Haiku passes about
  95 % of the tasks and native Luna about 30 %. The goal was about 50 % for
  Haiku. Fair tasks of 5–15 tool calls with deterministic checks do not
  stretch Haiku 5.5 at low effort ([set v1](#set-v1), [set v2](#set-v2)), so
  the Haiku tier can show a loss but hardly a gain. A family of deliberately
  hard tasks that brings Haiku near 50 % is future work.
- **A run takes about two hours.** A full run of v2 needs at least 112
  minutes at the default `JOBS=6`, before any wait, not the hour first aimed
  for ([runtime](#aa-check-of-v2)).
- **The best verdict is INCONCLUSIVE, by design.** The A/A run of v2, Aether
  1.1.0 against itself, passed the gate on both tiers, and its overall verdict
  was INCONCLUSIVE. An off-target set runs only with `ALLOW_OFF_TARGET=1` and
  can show a FAIL, but never pass a release, so the command exits 1 even when
  the gate passes. Read the report: a FAIL on either tier stops the release,
  and "INCONCLUSIVE (gate passed on an off-target set)" is the good outcome.

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
generators and their reference solutions. Each trial also gets its own XDG
cache and state, and, for Aether arms, its own daemon, config, data directory
and socket. Its TMPDIR is a fresh, short directory under `/var/tmp`
(`RELEASE_BENCH_TMP_ROOT`), removed when the trial ends. A short TMPDIR matters
because harnesses bind Unix sockets under it, and socket paths must fit in 108
bytes. Trials therefore run side by side, 6 at once by default (`JOBS`), a
level this 12-core host takes beside other work.
A trial starts only while the 1-minute load is at most `LOAD_MAX` (16) and at
least 2 GB of memory is free. A trial that waited for budget headroom checks
the host again before it starts, since the load may have risen meanwhile.

Every (task, repeat) is one block: its arms run back to back, in a shuffled
order. Within each model tier, `old` runs before `new` in exactly half of the
blocks, alternating, so neither binary systematically gets the warmer cache or
the quieter minute.

Caps per trial:

- A 600 s timeout.
- A spend allowance of USD 0.50 on Haiku arms and USD 0.15 on Luna arms. The
  harness's own cap applies where it has one: `--max-budget-usd` for Claude
  Code, `cost_budget` for Aether on Claude, Archon's item budget. Codex and
  Aether on Codex have none, so every arm also runs under a spend guard. The
  guard reads the trial's usage as it is written (Claude Code's stream,
  Codex's session logs, Aether's event store), prices it on the run's pinned
  table each second, and stops every process of the trial at the allowance.
  Codex writes each subagent's usage to a session log of its own, which names
  the root's session in its first line, so the guard follows those logs too,
  and the trial's final cost adds them to the root's. A native Codex trial
  whose root session log cannot be found stays unpriced.
  A stopped trial counts as a failure of its arm. Unknown spend is not taken
  as zero: when the guard has no usage to read for 30 s (Codex's session log
  cannot be found, say, or the store cannot be read), it stops the trial too.
  That is a harness failure on an Aether or Archon arm, and an infrastructure
  error on a native arm, where only the bench's reading failed.
- A limit on the agents that call a model at once, installed in each arm's
  harness:

  | Arm | Agents at once (solo task, delegation task) | Installed as |
  | --- | --- | --- |
  | Aether `old`, `new` | 1, 5 | `[scheduler] max_concurrent_turns` in the trial daemon's `config.toml`, engine turns included |
  | Archon | 2, 6 (the orchestrator counts) | the same key in the operator config Archon seeds its Aether from |
  | Claude Code | 2, 5 | `CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS` of 1 or 4, beside the root |
  | Codex | 2, 5 | `max_concurrent_threads_per_session` of 1 or 4, under both `agents` and `features.multi_agent_v2` (Codex reads the one its multi-agent version uses) |

  The native CLIs take no subagent limit below 1, so a native trial may run
  two agents even on a solo task. Over the limit, Aether queues the turn, and
  the native CLIs refuse the subagent.
- Before starting a trial, the run reserves the trial's worst case against
  `BUDGET`: the allowance plus two calls at the model's largest possible
  price for each agent the limit above lets it run at once (a full context
  window and a full output). Those two calls are one finished between two
  polls and one in flight at the stop. On Haiku that is USD 1.54 for a solo
  Aether trial and USD 2.58 for a solo native or Archon trial; on Luna, USD
  0.35 and USD 0.54. Delegation tasks reserve more, up to USD 6.74 for Archon
  on Haiku. The run stops starting trials when a reservation no longer fits.
- A trial whose usage cannot be priced is charged its whole reservation. The
  exception is an Aether turn that recorded no tokens, because it failed or
  ended before any model call (a sandbox that did not start, say). Such a turn
  spent nothing and is priced at zero. The trial's cost then comes from every
  agent's cumulative usage in its event store, and `price_note` in its
  `result.json` says so.

An interrupted run resumes when started again with the same `--run-id`. The
run journals every trial start before the trial begins (`attempts.jsonl`). A
resumed run moves the directory of a trial that never recorded a result aside
as `<name>.abandoned-N`, and charges that attempt its whole reservation.
A start torn by a crash, an unterminated last journal line, never reached
its trial: the resumed run cuts it before journalling new starts.

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
    bootstrap over tasks. Only the (task, repeat) cells that both arms counted
    are compared, so an outage on one arm never leaves the other averaging
    repeats it lacks. Each task's matched repeats are averaged first, so the
    task is the unit of resampling. Cost is paired the same way.
  - An exact McNemar test on the discordant pass/fail pairs.
  - The cost ratio new/old, with its bound.
- **Drift:** `old` against `native`, for reference only.
- **Per family:** pass rates by task family.

Trials end in one of three ways:

- **Counted:** graded by the hidden checks.
- **Harness failure:** counted as a failed trial. On Aether and Archon arms
  this covers a crash, an exit without model output, a telemetry or cleanup
  error, or a root left blocked. On every arm it covers a stop by the spend
  guard. The harness under test answers for all of these, so a candidate that
  crashes loses those tasks.
- **Infrastructure error:** excluded. That is a failure of the bench's own
  steps, or a provider outage named in the error (overloaded, rate limit, 429,
  5xx, connection reset). On the native drift reference, a CLI that produced
  no output at all is one too.

The verdict for a tier is:

- **FAIL** when, at 95 % one-sided confidence, the candidate's pass rate is more
  than 5 points below the old release's, or its cost is more than 15 % above it.
- **INCONCLUSIVE** in any of these cases:
  - fewer than 20 tasks were paired;
  - more than 10 % of the old arm's or of the candidate arm's trials hit an
    infrastructure error (each arm is checked on its own);
  - the planned matrix did not finish: a tier that never ran, a stop at the
    budget, or a repeat that ran only partly;
  - fewer than 95 % of the paired tasks are priced on both arms, so the cost
    gate cannot be judged.

  A FAIL on accuracy stands without prices. A FAIL on an unfinished matrix
  stands only under `--early-stop`, whose looks are planned for it.
- **PASS** otherwise.

The overall verdict is FAIL when any tier fails. It is PASS when every tier
passes and every planned arm ran all its trials, the native and Archon arms
included: an unfinished drift reference blocks the PASS, and the report lists
what is missing. It is INCONCLUSIVE otherwise, also when no tier ran both an
old and a candidate arm. Arms left out on purpose (`ARMS=...`) are not
planned, so they block nothing. The make target exits 0 only on PASS. A point estimate beyond a margin that is not
significant is listed as a warning.

`EARLY_STOP=1` looks at the gate when half and when three quarters of
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
| `SET` | `bench/release-set/v2` | frozen task set |
| `ARMS` | `all` | `all`, `haiku`, `luna`, or comma-separated arm keys |
| `JOBS` | 6 | trials at once |
| `LOAD_MAX` | 16 | start no trial above this 1-minute load |
| `BUDGET` | 20 | USD API-equivalent; stop starting trials beyond it |
| `REPEATS` | 1 | runs of every (task, arm) |
| `ARCHON_BIN` | unset | an `archon` executable; adds an Archon arm per tier (below) |
| `EARLY_STOP` | unset | `1` turns on the interim looks |
| `ALLOW_OFF_TARGET` | unset | `1` runs a set whose calibration missed its targets; its verdict is then at most INCONCLUSIVE |
| `EXTRA` | unset | more `release_bench.py run` options, such as `--limit 10` |

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
| refactor | `exporters` | merge three or four copy-pasted exporters into one shared `format_row` while every exporter keeps its exact output, quirks included |
| delegation | `workstreams` | fix four unrelated packages, each against its own README; the prompt asks for one sub-agent per package |
| longctx | `protocol` | fix a decoder against a 57 KB protocol spec whose errata, in the middle of the document, override the message table |

The last three are the harness slice. Their checks are deterministic like the
others:

- `exporters` diffs each exporter against the starter on generated rows.
  Parsing the file's syntax tree, it also checks that every exporter calls
  `format_row` and that none formats a field itself.
- `workstreams` passes only when all four packages match their READMEs. Its
  spec allows delegation, so Aether and Archon arms run without the
  work-alone instruction, and the record keeps how many agents each trial
  started.
- `protocol` checks each planted defect, and that every message type in the
  spec decodes.

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
make bench-calibrate FREEZE=bench/release-set/v3   # NEW's price table; CAL_BUDGET=25, CAL_REPEATS=3
```

This runs `COUNT` (200) verified candidates `CAL_REPEATS` times on the two
native arms (`TEMPLATES=a,b` limits the templates). A task is eligible when it
never hit an infrastructure error and native Haiku passed it in 20–80 % of its
runs. A task Haiku always or never solves cannot show a change on its tier. The
run then trims the eligible tasks (80–120) so the mean native Haiku pass rate
lands near 50 % and Luna's near 30 %, preferring tasks whose own Haiku rate is
nearest 50 %.

Last, while a release run of the set would outlast an hour, the slowest tasks
are dropped, down to the minimum size. A release run's length is estimated from
each task's mean native solve time in calibration. Each tier runs three arms;
in the v2 A/A the Aether arms took as long as native on Haiku and 1.45 times as
long on Luna. The sum is then spread over 6 workers that never wait. That is a
lower bound: load and budget waits come on top.

Last, no template may hold more than 20 % of the final set. While one does,
its task farthest from the Haiku target is dropped. The cap is a share of the
set as chosen, not of the 120 maximum, so it can take the set below 80 tasks.
A pool of fewer than five templates can never meet it.

The set is **accepted** when it holds at least 80 tasks, no template holds
more than 20 % of them, its mean Haiku rate is within 10 points of 50 %, its
mean Luna rate is within 25–35 %, and that estimate is at most 60 minutes. Manifests record each task's mean solve times
from v3 on. A task carried from v1 or v2, which record none, makes the estimate
unknown, and the set is not accepted.
`calibration.md` lists every reason a set is not accepted. Calibration freezes
only an accepted set, unless `ACCEPT_OFF_TARGET=1`. A set frozen that way
records `accepted: false` in its manifest, and `make bench-release` refuses it
without `ALLOW_OFF_TARGET=1`. Even then, the overall verdict is at most
INCONCLUSIVE, so an off-target set can show a FAIL but never pass a release.

Two options save a full recalibration (pass them through `EXTRA`):

- `--carry bench/release-set/vN` adds that set's tasks to the pool, with the
  rates its calibration recorded, so only new or changed templates are run.
  Tasks of the templates named in `TEMPLATES` are left out, since those are
  measured afresh.
- `--haiku-band LOW,HIGH` changes the eligible Haiku band. Acceptance still
  judges the set against the targets.

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

Superseded by the [A/A check of v2](#aa-check-of-v2).

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

### Set v2

Set v2, the default since 2026-10-18, replaces v1. It is **not accepted**: its
mean native Haiku pass rate is 95 %, far from the 50 % target, so
`make bench-release` runs it only with `ALLOW_OFF_TARGET=1`, and its overall
verdict is at most INCONCLUSIVE. v1 predates the acceptance check, so it is
refused the same way.

v2 was built by `make bench-calibrate` with `COUNT=40 CAL_REPEATS=2`,
`TEMPLATES=exporters,workstreams,protocol,versions`, `ACCEPT_OFF_TARGET=1` and
`EXTRA="--carry bench/release-set/v1 --haiku-band 0,1 --min-size 100"`:

- **Measured:** 40 new candidates, 10 seeds of each of the three harness
  templates and of the regenerated `versions` template, whose doc now states
  its prerelease boundaries. Each ran twice on both native arms: 160 trials,
  USD 1.97 API-equivalent, 48 minutes, no infrastructure error.
- **Carried:** v1's 81 tasks outside `versions`, with the rates v1's
  calibration recorded for them.

Native outcomes of the measured templates:

| Template | Haiku passed | Luna passed | Mean time |
| --- | ---: | ---: | ---: |
| `exporters` | 15 of 20 | 11 of 20 | 32 s |
| `protocol` | 20 of 20 | 6 of 20 | 34 s |
| `versions` | 20 of 20 | 0 of 20 | 106 s |
| `workstreams` | 20 of 20 | 19 of 20 | 35 s |

On 16 of its 20 `workstreams` runs, Claude Code started four sub-agents, one
per package. The template does exercise delegation, but Haiku solves it either
way.

Only 8 of the 121 tasks in the pool fall inside the 20–80 % Haiku band: three
`exporters`, three `query` and two `ranges` tasks. That is far short of a set.
The band was therefore widened explicitly to 0–100 %, and the selection
trimmed the pool toward the Luna target. The result has 110 tasks, a mean
native Haiku pass rate of 95 % and a mean Luna pass rate of 30 %. The slice
from the new templates is 8 `exporters`, 7 `protocol`, 4 `workstreams` and 10
`versions` tasks. Its [calibration.md](../bench/release-set/v2/calibration.md)
records the shortfall.

The finding of v1 mostly stands. Fair tasks of 5–15 tool calls, with
deterministic checks, rarely stretch Haiku 5.5 at low effort. Neither
delegation nor a 57 KB spec with errata did, and exact specs did not either.
The invariant-preserving refactor is the exception: Haiku passed 15 of 20
`exporters` runs in calibration, and 3 of the set's 8 `exporters` tasks fall
inside the band. One such family cannot carry a set. A Haiku tier near 50 %
needs many tasks Haiku consistently fails yet a reference solves. That is a
new, deliberately hard family, left to a follow-up decision.

### A/A check of v2

`make bench-release` ran with Aether 1.1.0 as both `OLD` and `NEW` and
`ALLOW_OFF_TARGET=1`. It completed all 660 trials (110 tasks × 6 arms,
`JOBS=10`) for USD 9.12 API-equivalent, with no infrastructure error and no
harness failure. The gate **passed on both tiers**. The overall verdict is
INCONCLUSIVE, as it must be on an off-target set. The full report is
[release-bench-2026-10-18-aa-v2.md](release-bench-2026-10-18-aa-v2.md).

| Arm | Pass rate [95 % CI] | Check fraction | Cost / trial | Median time |
| --- | --- | ---: | ---: | ---: |
| `haiku/native` | 95 % [91 %, 99 %] | 99 % | USD 0.020 | 48 s |
| `haiku/old` | 95 % [90 %, 98 %] | 99 % | USD 0.021 | 49 s |
| `haiku/new` | 95 % [91 %, 99 %] | 99 % | USD 0.021 | 48 s |
| `luna/native` | 35 % [25 %, 44 %] | 74 % | USD 0.005 | 23 s |
| `luna/old` | 54 % [45 %, 63 %] | 87 % | USD 0.007 | 33 s |
| `luna/new` | 44 % [35 %, 53 %] | 84 % | USD 0.006 | 29 s |

- **Haiku gate:** pass rate +0.9 points [−2.7, +4.5]; McNemar 3 against 4,
  p = 1.000; cost ×0.99 [×0.85, ×1.17].
- **Luna gate:** pass rate −10.0 points [−19.1, −0.9]; McNemar 24 against 13,
  p = 0.099; cost ×0.84 [×0.75, ×0.95]. The point estimate is beyond the
  5-point margin, so the report warns. The bound does not establish a drop
  beyond the margin, so the gate does not fail.

Both Luna arms ran the same binary, yet 37 of the 110 tasks came out
differently on them. Luna's outcome on a task varies from run to run, and the
v1 A/A run showed +2.2 points the other way. At one repeat, the Luna bound's
half-width is about 9 points, so the tier confirms a drop beyond 5 points
only when the true drop is about 14 points or more. Haiku's tier is far
tighter, but it can show only losses. `REPEATS=2` narrows both bounds, at
twice the cost and time.

On the harness slice, every arm passed every `workstreams` task. Both Aether
arms and Claude Code started 2–5 sub-agents on each. Codex started 3 on
`workstreams-7` and 4 on `workstreams-8`, though the report counts none: this
run did not yet read Codex's subagent logs. Their USD 0.014 is missing from
the `luna/native` cost (USD 0.569 of USD 0.583, still USD 0.005 a trial) and
from the USD 9.12 below. Native Luna is the drift reference, so the gate is
unaffected.
Every Haiku arm passed all 7 `protocol` tasks; the Luna arms passed 1 of 21
trials. On `exporters`, every Haiku arm passed 5 of 8, against 1 to 6 of 8 on
the Luna arms.

Two trials of the first leg were unpriced and charged their reservations:

- `workstreams-5` on `luna/old` had one root turn that ended with no tokens.
  Such turns are now priced at zero (above), and its store prices the trial
  at USD 0.033.
- `pricing-4` on `luna/new` hit Aether's own token budget, which left its one
  turn non-terminal. Its recorded usage (USD 0.060) is a floor, so it stays
  unpriced.

The USD 9.12 counts both trials at their store prices, and the abandoned
attempt at USD 0.12.

**Runtime.** The trials took 670 trial-minutes, so 10 at once with no waiting
would take 67 minutes. `pricing`, `versions` and `query` take 55 % of that.
The host gate never held a trial. The budget did. A reservation is a trial's
worst case: USD 1.54 for a Haiku trial and USD 5.70 for Haiku on a delegation
task, about 75 times a typical trial's cost. Trials are admitted strictly in
schedule order, so one reservation that does not fit holds back every trial
behind it. Late in the run, actual spend plus reservations in flight nears
`BUDGET`, and fewer trials run at once:

- The first leg, at `BUDGET=14`, also charged its two unpriced trials USD 1.13
  each. It was down to one trial at a time after 461 trials and 64 minutes,
  and was interrupted there.
- The second leg resumed at the default `BUDGET=20` and ran the last 199
  trials in 31 minutes. Its free workers waited 25 worker-minutes for budget
  headroom.

Native and Archon trials have since reserved for two agents at once (see the
caps above), USD 2.58 rather than 1.54 on Haiku, which adds to that pressure.

The whole run took 95 minutes at `JOBS=10`. At the default of 6 workers, the
same 670 trial-minutes need at least 112 minutes, before any wait. **v2 misses
the hourly target**: it needs nearly two hours. Haiku's three arms take 66 % of
the trial time. Calibration now checks this target (above). With v2's mix of
Haiku and Luna time, a v3 of 80 tasks fits the hour only at about 85 s of
native Haiku and Luna time per task, against v2's 111 s. Adding workers does not cure
it, because this host stalls when heavy jobs overlap. `BUDGET=30` would leave
room for 10 Haiku trials plus a delegation trial to the end of the run. It does
not change what a run costs, only the worst case it may reach.

## The Archon arm

`ARCHON_BIN=/path/to/archon` adds `haiku/archon` and `luna/archon`. Each trial
runs its task through a private Archon whose Aether is the candidate binary
(`NEW`):

- **Isolated state.** The trial gets its own `ARCHON_CONFIG_DIR`,
  `ARCHON_DATA_DIR` and `ARCHON_SOCKET`, and an `AETHER_CONFIG_DIR` as the
  operator config Archon seeds from, holding only the trial's turn cap. Variables that could reach a live Aether
  are removed. The trial's `PATH` starts with a link to the candidate, so
  Archon's dedicated mode starts the candidate as its own `aetherd`, in the
  trial's data directory and beside the trial's socket.
- **One item.** `archon setup` and `archon daemon run --detach` start Archon.
  The workspace is registered as a repository that ships locally, and the task
  text is queued as one item of the `direct` pipeline. The item runs on the
  arm's model at low effort (the orchestrator too), with the arm's allowance as
  Archon's item budget. Review, report, proposal and merge are all off.
- **Result.** `archon queue wait` bounds the item by the trial timeout, and a
  timed-out item is cancelled. The newest run branch that holds commits
  becomes the graded workspace. Without one, the starting commit is graded.
  `archon daemon stop` then stops both daemons.
- **Usage.** Usage comes from every agent in the trial's Aether store,
  orchestrator included, priced on the run's table. The same store feeds the
  spend guard while the item runs.

The commands and their exit codes are kept in the trial directory as
`archon-commands.json`, and the item with its runs as `archon-item.json`. A
failed item, or a failing `archon` command, is graded on whatever the
workspace holds. It is a harness failure when no model output came first, and
so is a failure to stop.

A smoke run on 2026-10-18 used Archon 1.2.5 with Aether 1.1.0 as the
candidate. It ran one `batching` task of set v1 on `haiku/new` and `haiku/archon`
(`ARMS=haiku/new,haiku/archon EXTRA="--limit 1"`). Every `archon` command
exited 0, and the item's run branch was graded: 9 of 9 checks on both arms. The
Archon trial took 56 s and cost USD 0.020 over two agents (orchestrator and
stage), against USD 0.011 and 51 s for one Aether agent. The spend guard peaked
at USD 0.019, and both daemons stopped cleanly. The Archon arm is not part of
the A/A run below, to keep it within its budget.

## Tests

```sh
python3 -m unittest discover -s evals -p 'test_release_bench*.py'
```

These tests cover:

- the statistics: McNemar, the bootstrap, the gate (including incomplete
  matrices, per-arm outages, outages on different repeats of each arm,
  candidate-only crashes and missing prices), interim looks, and the overall
  verdict with an unfinished native or Archon arm;
- the pool: counterbalanced scheduling, budget reservations against runaway
  trials, and resume after an interrupt, also twice after a torn journal line;
- the spend guard, including unknown spend, a source that recovers within the
  grace period, stopping marked processes, and Codex subagents that spend
  while the root idles (in the guard and in the trial's final cost);
- the host gate, checked again when the load rises during a budget wait;
- trial isolation: a TMPDIR short enough for sockets, removed after a crash,
  and pricing of turns that never reached a model;
- calibration: the band, selection, the template share of the final set, the
  release-time estimate and acceptance;
- frozen-set integrity, arm caps and trial records;
- the agent limit each arm installs (Aether's config, Archon's seed, the
  native CLIs' settings) and that the reservation counts the same limit;
- the Archon launcher, against a fake `archon`;
- one generated task per template, and the harness templates' checks against
  partial and quirk-dropping solutions.

None of them calls a model. `scripts/ci.sh` runs them again on Python 3.11,
the oldest supported version, when one is installed (`python3.11` or uv's).
