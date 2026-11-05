# Noesis evals

Three small tools. Only the first two checkers run in CI, and none of them calls a model or the network.

## Parity runner

`parity_cases.json` maps every behavioural parity row to the Rust tests that prove it:
T1–T12 (teaching), S1–S4 (service), S5-G01…G15, S5-H1, S5-H2 and S5-SETTINGS (off by default and hidden answers), and X1–X10 (graph).
The X rows group the graph, extraction and store tests by behaviour. A row with no test must say why in `gap`.

```
python3 evals/noesis/parity.py --check   # default; reads files only, no cargo
python3 evals/noesis/parity.py --run     # runs the named tests with cargo, prints id -> pass/FAIL
```

`--check` verifies that each required id appears exactly once, that every listed `fn <name>(` exists in its file, and that no row is empty without a `gap`.
`--run` calls `cargo test -p <crate> <--lib | --test stem> -- <names>` once per target, honours `CARGO_TARGET_DIR`, and exits non-zero if any test fails or was not run.
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

LearnHarness is AGPL. Nothing from it is imported, vendored, copied, quoted or fetched here, and none of its code, prompts or assets may be committed to this repository. The harness only drives a checkout you already have.

## Quality eval (manual, never gated)

Probing efficiency, plan quality and question quality are not deterministic, so they are scored by a human or a subscription model you run yourself, and reported in the PR.
`quality/rubric.json` holds the rubric. Export transcripts as JSON (`{"id", "mode", "messages": [{"role", "phase", "text"}]}`), then:

```
python3 evals/noesis/quality/score_sheet.py transcript1.json transcript2.json > sheet.txt
```

The sheet is a blank scoring form with each transcript above it. The script makes no network calls.
