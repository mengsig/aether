# Noesis evals

Three small tools. Only the first two checkers run in CI, and none of them calls a model or the network.

## Parity runner

`parity_cases.json` maps every behavioural parity row to the Rust tests that prove it:
T1–T12 (teaching), S1–S4 (service), S5-G01…G15, S5-H1, S5-H2 and S5-SETTINGS (off by default and hidden answers), and X1–X10 (graph).
Each row quotes its behaviour and Noesis form from NOESIS.md §6 (the S5 rows quote ARCHITECTURE.md §g) and splits the behaviour into `clauses`.
A clause is quoted from the behaviour or the Noesis form, and has `tests` that assert it, a `gap` saying what is not proven, or both.
An optional `noesis` note says how Noesis differs for that clause; a row whose form is not `same` needs one, or a clause quoted from the form.

```
python3 evals/noesis/parity.py --check   # default; reads files only, no cargo; lists every gap
python3 evals/noesis/parity.py --run     # runs the named tests with cargo, prints id -> pass/FAIL
```

`--check` verifies that each required id appears exactly once, that every word of a behaviour is in one of its clauses, that every listed `fn <name>(` exists in its file, and that each `src/` test file is declared as `mod <stem>;` by its parent (a `#[path]` module has another test path).
It prints the number of gapped clauses and one `GAP <id>: <clause> -- <why>` line each; a gap is allowed but never hidden.
`--run` calls `cargo test -p <crate> <--lib | --test stem> -- --exact <full test paths>` once per target, so no other test runs, and honours `CARGO_TARGET_DIR`.
A nonzero cargo exit fails every test of that run, even those that printed `ok`, and the output tail goes to stderr. It exits non-zero if any test failed or was not run.
`python3 -m unittest discover -s evals/noesis -p 'test_*.py'` tests the checker and runs in CI.

## Differential harness (opt-in, never in CI)

For maintainers with their own LearnHarness checkout. It starts the upstream app unmodified from that directory, points it at a keyless OpenAI-compatible endpoint served by a scripted faux model on loopback, and drives one scripted conversation over the app's HTTP API.
It compares structure only (phase sequence, question validity, plan validation results, graph statements and premise sets), never ids or bytes, against `differential/expected/structure.json`, which is derived from Noesis behaviour.

```
python3 evals/noesis/differential/run.py --learnharness /path/to/your/checkout
LEARNHARNESS_DIR=/path/to/your/checkout python3 evals/noesis/differential/run.py
```

Without a checkout, or with a directory that is not one, it prints `SKIP: …` and exits 0. It needs no credentials and posts nothing anywhere.

Everything specific to upstream (start command, endpoints, state shape) lives in `differential/adapter.py`, with a numbered list of assumptions at the top.
Those are guesses about a black box: adjust them for your checkout, and set `LEARNHARNESS_START` if it does not start with `npm start`.
The launcher runs in its own session. Stopping signals that whole process group, even when the launcher already exited: SIGTERM, a bounded wait, then SIGKILL.
The faux model and the temp HOME are cleaned up even when a step or the stop fails, and the failure is still raised.

LearnHarness is AGPL. Nothing from it is imported, vendored, copied, quoted or fetched here, and none of its code, prompts or assets may be committed to this repository. The harness only drives a checkout you already have.

## Quality eval (manual, never gated)

Probing efficiency, plan quality and question quality are not deterministic, so they are scored by a human or a subscription model you run yourself, and reported in the PR.
`quality/rubric.json` holds the rubric. Export transcripts as JSON (`{"id", "mode", "messages": [{"role", "phase", "text"}]}`), then:

```
python3 evals/noesis/quality/score_sheet.py transcript1.json transcript2.json > sheet.txt
```

The sheet is a blank scoring form with each transcript above it. The script makes no network calls.
