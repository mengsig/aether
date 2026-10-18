# Evaluations

Experiments that hold Aether to the standard of native Claude: same model,
same tools, same tasks, compared on outcome, turns, tool calls, tokens,
cost and time. Live-provider runs spend real tokens and are separate from CI's
mock end-to-end checks. Some older comparisons use a minimal API loop rather
than a native harness; inspect each experiment's control before interpreting it.

Generated `results/` files are ignored local artifacts. Historical observations
below describe their dated revisions, not acceptance results for a release.

Setup once:

```sh
python3 -m venv evals/.venv && evals/.venv/bin/pip install -r evals/requirements.txt
cargo build --release -p aether
export ANTHROPIC_API_KEY=...        # or `aether login anthropic`
```

## Task parity

`evals/run_tasks.py` runs each task in `evals/tasks.json` on a fresh copy of
`evals/fixtures/pyproj` twice: once through the Aether daemon (`aether run`),
once through `evals/reference_agent.py`, a minimal manual tool loop on the
official Anthropic SDK with the same four tools and prompt caching. Success is
a shell check per task (tests pass, symbols present, tests not modified).

```sh
evals/.venv/bin/python evals/run_tasks.py --model anthropic/claude-opus-5-5 --runs 2
```

Read the table for: success rate parity, and Aether's input tokens per turn
versus the reference. Aether carries the harness policy, runtime state and
tool schemas on every turn; the cached prefix should absorb most of it.

## Compaction recall

`evals/compaction_recall.py` feeds a synthetic operations brief with N unique
facts over several turns, forces compaction, then asks questions whose
answers live only in the folded turns.

```sh
evals/.venv/bin/python evals/compaction_recall.py --model anthropic/claude-opus-5-5 --facts 40 --questions 20
```

Conditions: `aether` (structured, validated checkpoint), `full` (native Claude
with everything still in context: the upper bound), `summary` (native Claude
answering from a naive free-text summary). Recall is exact match on the value.

Results append to `evals/results/*.jsonl` with the raw answers so misses can
be inspected.

## Release bench

`make bench-release OLD=path/to/last/aether ALLOW_OFF_TARGET=1` runs the
frozen task set in `bench/release-set/` on Haiku 5.5 and GPT-6 Luna, each
natively, through the last release and through the candidate, and gates the
candidate on its pass rate and cost. Run it only before a release, never per
change or in CI. `make bench-calibrate` builds a new set from the generated
templates in `release_bench_families/`. See
[docs/release-bench.md](../docs/release-bench.md).

```sh
# No model calls: statistics, gate, pool, selection, frozen-set integrity.
python3 -m unittest discover -s evals -p 'test_release_bench*.py'
```

## Native harness and coordinator pilot

`harness_bench.py` compares the installed native Claude Code/Codex CLIs with
Aether single-agent, default-coordinator, and explicit two-worker workflows.
Unlike `run_tasks.py`, its native baseline is the real CLI, not the reference
SDK loop. Its fourteen fixtures cover journaled transactions and concurrency,
checkpoint recovery, dependency scheduling, resource leases, event-stream
consumer groups, a multi-module line server, a limit order book, a
spreadsheet engine, Decimal invoicing, SemVer ranges with a backtracking
resolver, and a small interpreter. Held-out graders are validated
against buggy starters and separate reference implementations.

```sh
# No model calls: runner, grader, accounting, and report regression tests.
python3 -m unittest discover -s evals -p 'test_harness_bench*.py'

# Paid pilot: fresh workspaces and isolated daemons, sequential trials.
python3 evals/harness_bench.py --runs 1 --timeout 900 \
  --binary target/release/aether --output-dir target/harness-pilot

# Matched low-effort models plus each installed CLI's native default.
python3 evals/harness_bench.py --cases concurrent,eventlog --runs 1 \
  --conditions native-sonnet-low,aether-single-sonnet-low,native-luna-low,aether-single-luna-low,native-claude,native-codex \
  --binary target/release/aether --output-dir target/harness-low-tier

python3 evals/harness_bench_report.py \
  target/harness-pilot/harness-bench-<timestamp>.json \
  --output-dir .lavish/harness-pilot

# One block of the lower-tier matrix: matched Haiku 5.5 and GPT-6 Luna cells,
# up to four trials at once, each pair in one block.
python3 evals/harness_bench.py --cases matchbook,gridcalc --runs 1 --seed 1 --jobs 4 \
  --conditions native-haiku-low,aether-single-haiku-low,native-luna-low,aether-single-luna-low \
  --binary target/release/aether --output-dir target/harness-lower-tier

# Accuracy, tokens, cost and wall time per model pair, from one or several blocks.
# --trials appends one line per trial.
python3 evals/harness_bench_compare.py target/harness-lower-tier/harness-bench-*.json --trials
```

`--jobs N` keeps every trial in its own workspace and isolated daemon; a
case's native and Aether trials of one model always share a block, so wall time
is comparable within a pair but is not an absolute latency. `--pilot` marks a
manifest as calibration: both reports refuse it, so a pilot can
never be counted. See `harness_bench_protocol.md` for the rules.

Beside the counted `grade`, a trial records `grade_by_test`: when the grader's
one process timed out, every hidden check run again in a process of its own.
It is not the counted grade; `harness_bench_compare.py --trials` prints it in
a column of its own. If it cannot be made, it is `null` with the reason in
`grade_by_test_error`, and the counted grade stands.

`--diagnostic` marks a manifest as an overhead diagnosis and is required for
the `diag-...` conditions (the task inside Aether's first-turn envelope run
natively, with and without the words "Effort low."; an Aether cell with
only `complete_task` and `fail_task`; a native cell without the CLI's own
coordination tools; a native and an Aether cell that load the machine
user's MCP connectors; and an Aether cell asked to finish by calling
`complete_task`; for GPT-6 Luna, a native cell under Aether's sandbox and
approval options, one with Codex's multi-agent feature off, one with the
task inside the envelope, an Aether cell that names a diagnostic binary, and
`diag-native-luna-low-repeat`, the counted native cell a second time in the
same block as a yardstick). Both reports refuse it too. From protocol version 2
no counted Claude cell loads those connectors on either side; manifests of
two protocol versions are never merged.
`harness_bench_transcript.py MANIFEST...` reads, from each trial's Claude Code
or Codex session log, the figures a manifest does not hold: first-request
input tokens, thinking or reasoning before the first edit, requests, tool
calls, test runs and what the first request carried. The [overhead diagnosis](../docs/harness-overhead-diagnosis-2026-10-10.md)
uses both.

`harness_bench_compare.py` pairs trials by case and run within one manifest,
never across manifests: run both sides of a pair in the same block, or the
paired differences read "0 pairs". Token and cost figures from incomplete
receipts (a timed-out trial, an Aether Claude trial without its process
receipts) are lower bounds: they enter no mean and are listed under the table.
`--trials` adds a line per trial (manifest file, case, run, cell, workflow,
hidden checks, tokens, cost, wall seconds) for a report whose manifests are
not kept beside it; a lower bound reads "at least" there, and a trial with
tokens but no price reads "unpriced".

The cost-first report leads with uncached input, output, and API-equivalent USD.
Mean accuracy is the mean fraction of hidden checks passed per graded trial;
`summary.md` holds Markdown tables by condition and by case.
Each row labels its basis (billed / estimated / mixed), with billed, estimated,
and unpriced unit counts. Cost means include only fully priced terminal trials
and show priced/total counts; unknown or partial costs are not zero. “Billed”
means reported dollar metadata, not a subscription invoice.

The runner captures `price_table` from its pinned binary's `list_models` using
an isolated daemon, including the binary SHA-256. Runtime estimates are retained;
unpriced Aether turns use their captured `engine.session` model, while native
Codex prefers the trial's own `resolved_metadata` model and falls back to
`manifest.native.codex.configured_defaults.model` (explicitly an estimated
configured default). Missing or ambiguous models remain unpriced. A list-price
estimate that replaces a partial billed amount keeps it as `billed_floor_usd`.

Rebuild an existing manifest from raw `events.json` / native JSONL without
running models or altering the original manifest or trial artifacts:

```sh
# Offline when the manifest already contains price_table:
python3 evals/harness_bench_reprice.py path/to/harness-bench.json
# Older manifests: fetch the matching pinned binary's inventory in isolation.
python3 evals/harness_bench_reprice.py path/to/harness-bench.json \
  --binary path/to/pinned/aether --scratch-dir target/reprice-scratch
# Or inject a saved table with --price-table path/to/price-table.json.
# --output-dir EXISTING_DIR keeps all writes away from a read-only source tree.
```

Output is `harness-bench.repriced.json`; existing outputs are never overwritten.
Binary hashes must match. Isolated price capture requires `XDG_RUNTIME_DIR` for
a short owned socket; it never uses the operator's live daemon, and its scratch
directory is removed once the capture succeeds. `--scratch-dir` without
`--binary` is an error. Offline re-pricing needs neither a daemon nor
credentials. Missing/malformed captures are marked with a usage error, not
carried forward as stale values, and the CLI exits nonzero. A failed receipt
reconciliation is reported separately as `claude_reconciliation_error`.

See [the protocol](harness_bench_protocol.md) before interpreting the results.
The runner pins its executable and input hashes, retains failed-trial artifacts,
and distinguishes finished workflows from merely correct files. Usage reports
include coverage; native receipts and durable Aether counters are overlapping
views, not additive costs. Native defaults, Aether overhead, and mixed-model
coordination are different comparisons. Small pilots do not establish a stable
ranking or actual subscription charges. All trial scratch remains in `target/`.
Run a bounded comparison on the committed `concurrent_v2` fixture with
`--cases concurrent_v2 --seed 17 --runs 3` and explicit `--conditions`.
`aether-solo-minimal-luna-low` is an opt-in tool-surface treatment; it does
not change the default condition set.
The [lower-tier comparison](../docs/harness-lower-tier-results-2026-10-10.md)
reports five counted runs of the matched Haiku 5.5 and GPT-6 Luna low-effort
cells on `matchbook` and `gridcalc`: 40 trials, one runner invocation per run
with seeds 1 to 5, `--jobs 8 --timeout 1200`, each in its own output
directory. Run 5 was added after runs 1 to 4 had been seen and is counted
whole. Its manifests are ignored local files under `target/`, so the report
carries the per-trial table, and the five-run means are computed from it.
The [same comparison under protocol version 2](../docs/harness-lower-tier-results-2026-10-10-protocol-2.md)
reports five more counted runs, 40 trials, with the same cells, cases, seeds
and settings, on the binary that no longer repeats the effort in an engine's
status line. Its five runs were fixed before the first and none was added.
The [counted test again](../docs/harness-lower-tier-results-2026-10-11.md)
reports five more counted runs under protocol version 2, 40 trials, seeds 6 to
10, on the same binary and settings, with paired statistics over those runs
and over all ten version 2 runs. Its runs were fixed before the first and none
was added.
The first measurement is never merged with the version 2 runs: the tools
refuse manifests of two protocol versions.
The [six-case comparison at medium effort](../docs/harness-bench-2026-10-11.md)
reports 72 counted trials of the matched Haiku 5.5 and GPT-6 Luna
medium-effort cells on `billing`, `semver`, `minilang`, `concurrent_v2`,
`eventlog` and `leases`: three runs with seed 393, one runner invocation per
model with `--jobs 2 --timeout 900`, under protocol version 2, with paired
statistics. Its rules were committed before the first trial. A [first
sitting](../docs/harness-bench-2026-10-10.md) of the same 72 trials was
measured under protocol version 1; the two are reported side by side and
never pooled.

## Model profiles (historical experiment)

The following describes the retired profile policy and its original experiment.
Current native engines do not receive these authored profiles; API profiles are
opt-in. These old native commands do not measure the current API-only feature.

VISION §4.4 makes the authored per-model-family model profile (the text
injected as the `model_profile` context layer, and appended to an engine
agent's system prompt) ship only with a recorded eval showing it helps or is
neutral. `evals/model_profiles.py` is that eval: it runs `evals/tasks.json`
on the same model twice, once with the default config (`profile`, profiles
on) and once with `[model] profiles = false` in `config.toml` (`plain`), each
in its own throwaway isolated daemon, and compares them.

```sh
evals/.venv/bin/python evals/model_profiles.py --model claude-code/haiku --runs 3
evals/.venv/bin/python evals/model_profiles.py --model codex/gpt-6-luna
```

`--model` is repeatable and/or comma-separated, so several families can run
in one invocation. Read the table for success rate, mean turns, tool calls,
input tokens/turn and cost per condition, and the `verdict` line underneath
each model: `helps`, `neutral` or `hurts`, decided on success rate first and
turns/tokens as a tiebreak.

Results land in `evals/results/model-profiles-<date>.json`.

### Which half of a profile a run measures

A profile has two halves, and an engine model only ever exercises one of
them: `ModelProfile::text(true)` drops the tool half, because a CLI engine
owns its own tools. So a `claude-code/...` or `codex/...` run measures the
guidance half and nothing else. The tool half goes out on the API path only.

The `mock-*` providers are that path without credentials: one per family,
all answering from `scripts/mock_openai.py` under the `openai-responses`
adapter, which is the adapter the GPT family ships on.

```sh
evals/.venv/bin/python evals/model_profiles.py \
  --model mock-claude/claude-mock,mock-gpt/gpt-5-mock,mock-codex/codex-mock --runs 3
```

| provider | family | protocol |
|---|---|---|
| `mock`, `mock-claude` | claude | openai-responses |
| `mock-gpt` | gpt | openai-responses |
| `mock-codex` | codex | openai-responses |

**A mock run is structural, not a model result.** The mock is a pattern
matcher, identical under both conditions, so its verdict can only ever be
`neutral`; what it proves is which profile text reached the wire, that the
whole API path carried it (tool schemas, tool call, tool result, next
request), and what the profile costs in input tokens per turn. It is not
evidence that a real GPT model does better with the GPT profile - no such
evidence exists here, and the ARCHITECTURE row says so.

### Reading a verdict honestly

Outcome decides the verdict, and at these sample sizes outcome is noisy: on
four tasks at n=3, one task flipping moves the success rate by 8 points.
Token columns are noisier still, because cache accounting can swamp the
handful of tokens a profile adds - a recorded run showing *fewer* input
tokens per turn under `profile` than under `plain` is measuring the cache,
not the prompt. Treat `neutral` as "no harm was detected at this sample
size", never as "measured to make no difference".

## CLI delegation boundary

`python3 evals/delegation.py --binary target/debug/aether --output result.json`
runs fresh and resumed three-child checks with Sol, Luna and Claude Haiku
in an isolated Aether daemon and a tiny read-only fixture. It checks durable
child records, explicitly selected models, explicit low effort, successful completion
and absence of failed tools. This spends real provider tokens. An engine
startup failure fails the evaluation; it is not treated as a successful
delegation or silently retried.

## Ordinary-language delegation acceptance

`natural_language.py` uses the requests in `natural_language_cases.json`
without adding tool names, provider IDs, or repair messages. Cases include
Codex asking for Sonnet reviewers and a deeper Sol → Luna low → Sonnet tree.
A “few” allows 2–4 branches; the checker verifies each parent relationship,
model connection, requested effort, completion and recorded work result. It
rejects extra generations, untracked children and failed tool calls. Reports
retain agent records and assistant messages for review; summary presence alone
does not establish the quality of a documentation review.

```sh
python evals/natural_language.py --output evals/results/natural-language.json
python -m unittest discover -s evals -p test_natural_language.py
```

Each live case runs in its own temporary daemon/workspace using local CLI
credentials and spends real tokens. `--case NAME` selects a case; `--timeout`
bounds it. A passing run is one observation, not a measured reliability rate.

### Native web access

`python3 evals/native_web.py --binary target/debug/aether --out results.json`
checks native Codex and Claude web searches using ordinary language, configured
network denials against a local probe server with a positive control, and a
Codex network command with approvals enabled. It starts
isolated daemons, records actual native tool calls and errors, and keeps the
providers' own web tools. These live checks require both CLI logins and Claude's
native sandbox dependencies (`bubblewrap` and `socat` on Linux). They do not
claim that a network scope is OS-level containment for every host tool.

`python3 evals/native_web_children.py --binary target/debug/aether --out children.json`
checks ordinary cross-provider delegation requests, inherited network scope,
requested child effort, completed Aether task trees, and actual native web
calls in every child. Tool errors fail the check. Inspect the recorded answers
for research quality; a native tool call alone does not prove a correct answer.

### Claude standard context

`python3 evals/claude_context.py --binary target/debug/aether --out context.json`
checks Sonnet, Fable, Opus, and the native default through isolated Aether
daemons. A transparent CLI wrapper records only native result metadata. The
check requires 200K windows, ordinary session reuse, a completed Aether
compaction that replaces the session, and recall of a fact afterward. It needs
the installed Claude CLI and subscription login. This checks the context
setting and a single compaction; it is not the repeated-compaction evaluation.

The September 8 Aether run passed all four models. The user's native default
retained its `fable[1m]` label, but Claude reported a 200,000-token window on
both fresh and resumed calls. Check the reported window, not the alias label.
The Fable case used two summarizer invocations before completing compaction;
this report does not claim every checkpoint succeeded on its first attempt.

## Mid-turn child steering

`steering_children.py` asks a coordinator to delegate a documentation review,
then sends an operator correction after the child starts a tool. It checks the
stored delivery/consumption events, runtime turn identity, requested output
file, and the fixture's recovery findings. Both provider directions are tested.

```sh
python evals/steering_children.py --binary target/release/aether --out evals/results/steering-children.json
```

If a coordinator chooses a network-denied child, the native Claude sandbox
requires its platform dependencies (on Linux, `bwrap` and `socat`). An optional
`--sandbox-bin` adds a directory containing those dependencies to the eval PATH.

The initial 2026-09-09 report retains a verifier bug (completed-turn counters
were compared during an active turn) and a Claude startup failure before tools.
The corrected verifier uses `agent.started`/`agent.turn_completed` events. In
`steering-children-before-authority-fix-2026-09-09.json`, Codex passed; Claude
consumed the correction in the same turn but explicitly refused it as an
injection attempt conflicting with the parent's brief. This is a real failed
steering outcome, not a successful delivery-only acceptance result. A prompt
and inbox provenance fix is being validated separately.

The next steering run uses a plain request to change focus and filename; the
inbox's unique message ID provides delivery correlation without an arbitrary
output token. It still requires the changed output file, restore/retention
findings, and one consumption within the same runtime turn.

### Codex bridge approval regression (2026-09-09)

The exact `terra-luna-hierarchy` request completed all three Terra children and
nine Luna-low grandchildren before and after the bridge approval setting change.
The before report records 25 native MCP approval rejections and five shell probe
failures. The after report records no native MCP approval rejections and three
shell probe failures. Both strict results remain **failed**; this comparison
supports the approval fix, not a clean natural-language acceptance claim.
Full trees and errors are retained in
`results/codex-bridge-approval-before-2026-09-09.json` and
`results/codex-bridge-approval-after-2026-09-09.json`.

The updated operator-correction report,
`results/steering-children-operator-corrections-2026-09-09.json`, passed all four
cases (Terra, Sonnet, Spark, Haiku) on the combined approval/steering build.
Each has exactly one delivery and consumption, a corrected report observed at
the end of the steered turn with a modification time no later than that event,
and a completed task. Spark used one additional task-closing turn. The report
content was also reviewed against the fixture's backup/recovery gaps.

`results/steering-children-completion-turn-check-2026-09-09.json` retains an
intermediate evaluator that required task closure in the same turn, conflating
steering with completion bookkeeping. The current check proves the correction
was applied during the turn and records extra completion turns separately;
it still requires the task to complete. These runs do not establish restart,
fan-out, API-provider, or UI acceptance for the broader steering issue.

The first full pre-push check exposed an integration regression in the sentence
form of the operator marker: it became part of the visible user body, prevented
queued-message reconciliation in plain mode, and triggered the mock provider's
delegation keyword. The marker now lives only in the inbox header. The context
regression fails with the sentence form, and the UI test preserves both clean
message bodies and marker-like text actually typed by a user. Live steering is
rerun whenever this provider-facing format changes.

The header-marker rerun in
`results/steering-children-header-marker-2026-09-09.json` passed all four live
cases with the final prompt format. Each correction took effect during its
runtime turn; Spark again needed one additional task-closing turn. The full
mock end-to-end suite also passed, including clean user text and EOF exit in
plain mode.

## Cancelling working native children

`python3 evals/cancel_children.py --binary target/release/aether --out cancel.json`
asks for a Luna or Haiku documentation reviewer using ordinary language, then
cancels its supervisor while the child is running a tool. It verifies exactly
one durable cancellation request and completion, both agents ending Cancelled
exactly once, inherited caller attribution, and event ordering. Use
`--sandbox-bin` when native Linux sandbox dependencies are supplied separately.
This checks live cancellation, not crash recovery; injected store-failure and
restart coverage lives in the runtime kernel tests. The live Spark and Haiku cases passed on a4b5747;
`results/cancel-children-2026-09-09.json` records both complete event streams
and final agent states. Both trees ended Cancelled with exactly one event per
agent and the original caller attribution.

Add `--via-agent` to ask the supervisor to stop the reviewer in ordinary
language. That mode requires exactly one shared `cancel_agent` call, the
supervising agent as the recorded caller, a cancelled child, and the supervisor
settling back to waiting. It complements the direct operator cancellation case.

The final direct and supervisor-driven cancellation runs passed for Spark and
Haiku on the binary that exposes `cancel_agent`. See
`results/cancel-children-final-2026-09-09.json` and
`results/cancel-agent-natural-language-2026-09-09.json`. The supervisor mode
recognizes the actual MCP-prefixed tool name and observes a full second in
Waiting to avoid sampling an immediate task-wake transition. The initial
checker report is retained in `results/cancel-agent-initial-check-2026-09-09.json`:
it missed the prefixed name and sampled one supervisor between completion and
its follow-up turn, although both child cancellations and caller attribution
were correct.

## Schema 9 upgrade and rollback drill

`python3 evals/schema_upgrade.py --old-binary /path/to/schema8/aether --new-binary /path/to/schema9/aether --out upgrade.json`
creates three paused agents, their tasks, and an unread message using the old
binary, verifies a SQLite backup, upgrades through the new daemon, checks that
existing records remain, verifies that the old daemon refuses schema 9 without
changing the store, and restores the backup under the old daemon. The fixture
names the configured Codex connection but starts no model turns. The recorded
run passed in `results/schema9-upgrade-2026-09-09.json`. This small fixture does
not replace a current backup of the user's store before deployment, or the
broader crash/recovery and backup tooling requirements in phase 8.

## Account usage

`account_usage.py --binary target/release/aether --out results/account-usage.json`
runs one greeting through Luna and Haiku in a throwaway daemon, then checks
that each native provider's full usage report matches its durable quota event
and both reports remain available together in the same daemon.
It records the windows actually reported rather than assuming every account
has two limits. Use `--sandbox-bin` for separately supplied native sandbox
dependencies. It spends one small model turn per provider for validation;
opening the product's `/usage` window does not start a turn.

## First-turn delegation

`python3 evals/natural_language.py --binary /path/to/aether --cases evals/first_turn_cases.json --output /tmp/first-turn.json`
asks Haiku and Luna to start three researchers using ordinary language.
The historical runs below used Spark and Terra, since retired.
In addition to completed work, inherited models and zero failed tool outcomes,
these cases require three durable direct children between the coordinator's
turn-one start and end. Later turns and grandchildren cannot satisfy that
requirement. Reports retain lifecycle events and the tested binary hash.
Use `--repeat 3` for three independent trials of each selected case; every trial
uses a fresh daemon and workspace.
`python3 -m unittest discover -s evals -p test_natural_language.py` checks the
verifier's rejection cases. A passing fixture test is not a live provider pass.

`results/first-turn-before-2026-09-09.json` retains the initial checker race:
it stopped on waiting state before consuming the turn-completed event.
The corrected baseline in `results/first-turn-boundary-check-2026-09-09.json`
observed Spark creating zero children while doing the reads itself. Haiku
created three direct children during turn one but failed a shell probe for
an `mcp` executable. Neither baseline is an acceptance pass.

`first-turn-prompt-check-2026-09-09.json` records Haiku passing after the policy
change. Spark created three tracked children, but its usage limit prevented
completion; it is not a pass. `first-turn-terra-check-2026-09-09.json` records
Terra passing the same request. These runs precede the native timing fix; the
final-binary check must provide fresh evidence for that change.

`first-turn-final-check-2026-09-09.json` did not pass: a Haiku child used
`result` instead of the required `summary`, and Terra created a fourth dummy
child. These failures remain recorded; the subsequent policy clarification
requires a separate evaluation and does not retroactively make them passes.

The next repeated run, `first-turn-repeated-check-2026-09-09.json`, passed all
three Terra trials and two of three Haiku trials. The third Haiku trial tried
to locate a completion tool as a shell executable. That motivated loading the
Aether MCP server's schemas upfront in Claude, using its documented per-server
`alwaysLoad` setting. Fresh and resumed argument construction has a regression
that fails without the setting.

`first-turn-eager-tools-2026-09-09.json` passes all three Haiku and all three
Terra trials after that change: exactly three direct children created in turn
one, completed tasks, inherited models, and no failed tool outcomes. Reports
also retain observed native tool durations. These six small trials establish
this regression check, not general prompt parity or arbitrary hierarchy reliability.
Spark remained quota-blocked in those trials. The user subsequently clarified
that Spark is optional; available Codex models such as Terra or Luna satisfy
the Codex validation requirement.


## Recurring scheduling

Run the ordinary-language timer and daemon-restart check with available native
models:

```sh
python3 evals/scheduling.py --binary target/release/aether \
  --output evals/results/scheduling.json \
  --model claude-code/haiku --model codex/gpt-6-luna
```

The evaluator asks each agent to read README.md every ten seconds for two
deliveries. It verifies a single durable timer, pauses after the initial turn,
restarts an isolated daemon, checks the unchanged paused state, resumes it, and
requires two completed title reports, two deliveries, exhaustion, and no failed
tool calls. It stores the binary hash with the results. Delivery alone is not
counted as successful completion of the requested reading task.

The initial Haiku check paused during its setup turn and later found a changed
timer snapshot; that failed attempt remains in `scheduling-initial-2026-09-09.json`.
The corrected check pauses after the agent is waiting; its Haiku result is in
`scheduling-haiku-2026-09-09.json`. Luna passed the initial check. These small
checks do not establish arbitrary recurring-task reliability or soak behavior.

The final evaluator run, `scheduling-2026-09-09.json`, passes both Haiku and Luna
on binary SHA-256 `256679bd6caf3bcf50c5bb07d9c5934621f213dcf3c4db9284bd98575626d4c2`.
The authenticated TCP command smoke check also covers create, inspect, filtered
listing, history, pause/resume/cancel, and rejection of a cancelled resume.

## Optional validation models

Other models can be exercised by passing their IDs to the `--models` option of
`delegation.py` or `account_usage.py`. Steering uses Sol, Sonnet and Haiku as
its default cases and also offers `--case luna-child`; cancellation defaults to
Luna and Haiku. Model-specific historical results remain unchanged, including
those recorded on the retired Spark and Terra. These commands spend provider
tokens when actually run.
