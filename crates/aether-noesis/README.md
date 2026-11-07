# aether-noesis

Noesis is Aether's learning service. It teaches through questions, a
prerequisite plan, explanations and retrieval practice, and keeps its own
state in `noesis.db` in Aether's data directory.

## Off by default

`noesis.enabled` is `false` unless you set it. While it is off, Noesis opens,
creates, registers and starts nothing: `noesis.db` is not created, and no
worker, timer, tool or role exists. `Noesis::start_if_enabled` checks the
`[noesis]` settings and returns `None` without touching the data directory.

## What this crate holds

- `settings`: the `[noesis]` keys, their defaults and ranges, and the
  prerequisite rules that turn stored settings into effective ones (a switch
  counts only when the switches it depends on count).
- `store`: `noesis.db`, kept private to its owner (an existing file and its
  `-wal`/`-shm` are set to 0600, symlinks are refused), with migration N1
  (sessions, the messages index, questions and problems, plans, the graph
  ledgers, coverage and jobs) and N2 (the learner tables).
- `learner`: the learner model. `evidence` is append-only (a correction is
  a new row), and `concept_state` is a cache rebuilt only from it by a pure
  derivation of the concept's state, strength, review time, calibration and
  open misconceptions. The learner tables share only proposition ids with
  the graph, as plain values, so deleting the graph leaves them intact.
- `budget`: daily budgets and concurrency caps for interactive and
  background calls. A call is admitted before it is made, so a budget of 0,
  or one already used up, refuses the call before any model sees it. Spend is
  booked in whole micro-USD and charged to the day that admitted the call.
  An unset model slot takes the first ready subscription model and never a
  model billed per request; such a model is used only when a slot names it
  and `noesis.billing.allow_api = true`. This crate stores and logs no credentials:
  models are named by `provider/model` id only.

## Provenance

Noesis is a clean-room implementation, under the MIT licence like the rest of
Aether. It reproduces the behaviour of LearnHarness, an AGPL-3.0-only
learning app, from a written behavioural specification only. No LearnHarness
source code, prompt text, schema text, tests or assets were copied,
translated or paraphrased into this crate, and its authors did not consult
them. No LearnHarness code or text was used. Its prompts, when added, are
written from the general teaching literature (retrieval practice, worked
examples, spacing, interleaving, eliciting misconceptions and confidence
calibration).
