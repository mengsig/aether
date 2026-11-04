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
- `store`: `noesis.db`, created private to its owner, with migration N1
  (sessions, the messages index, questions and problems, plans, the graph
  ledgers, coverage and jobs).
- `budget`: daily budgets and concurrency caps for interactive and
  background calls. A call is admitted before it is made, so a budget of 0,
  or one already used up, refuses the call before any model sees it. A model
  billed per request is used only when `noesis.billing.allow_api = true`; it
  is never a silent fallback. This crate stores and logs no credentials:
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
