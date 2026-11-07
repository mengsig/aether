# Frontend performance budgets

The fixtures, scenarios and budgets of vNext task H2. Run them with `make web-perf`, or with
`npm run perf` in `web/` (`npm run perf:ci` checks only the host-independent measures, as the
`web-perf` workflow does on every pull request). `make ci` does not run them.

## Fixtures

All are generated from a seed (`fixtures/random.ts`, mulberry32), so no blob is committed and every
host measures the same data:

| Fixture | Size | Generator |
| --- | --- | --- |
| Fleet | 5,000 agents in runs of 1 to 24, a third Archon runs | `fixtures/fleet.ts` |
| Transcript | 20,000 entries: prompts, Markdown, reasoning, tool calls and results, attachments | `fixtures/transcript.ts` |
| DAG | 2,000 items in 40 initiatives, 4,027 typed edges (seed 2000) | `fixtures/dag.ts` |
| Delta load | 10 s at 60 fps: 3 streams × 40 text deltas/s, a commit every 2 s per stream, 20 fleet row updates/s | `fixtures/deltas.ts` |

## Scenarios

`scenarios/*.perf.tsx` mount the real screens (fleet, conversation, DAG workspace) under a real core
app in happy-dom, with the store committing only when a scenario runs a frame. Updates go through
core's own reducers (`aether-reduce.ts`). Each measure is:

- **counts** (`enforce: ci`): React commits, rows rendered (counted by wrapping the one component each
  row draws), DOM elements, DOM mutation records, layouts and canvas frames. These are the same on
  every host for the same lockfile.
- **bytes** (`enforce: ci`): `web/dist` sizes, raw and at gzip -9 (`bundle.ts`).
- **wall time and heap** (`enforce: local`): the median of 7 timed repeats after a warm-up, and the
  heap a mounted screen keeps after a full GC. They hold only on the host in `budgets.json`.

The numbers come from React's development build, which `act` needs, in happy-dom, which has no
layout or paint. They are regression guards for the JavaScript work, not absolute browser timings;
`dev/archon-dag-bench.ts` measures DAG frames in Chromium.

## Budgets

`budgets.json` holds, per measure, the measured median of the calibration runs (six on the budget host) (`measured`), the
run-to-run noise allowed (`noise`, 0 for counts and bytes; for wall time and heap the runs' spread, (max − min) / median, rounded up to 5%, and at least 20% for wall time and 10% for heap, since a few runs on a shared host miss its slowest moments),
the budget (`measured × (1 + noise)`) and why the measure matters. `run.ts` fails when a measure
is more than `tolerance` (10%) over its budget, when a budgeted measure was not measured, or when a
measure has no budget.

After an intended change, recalibrate on the budget host and review the diff:

```sh
sh ../scripts/ci_slot.sh node perf/run.ts --runs 6 --calibrate
```

It prints each measure across the runs and the budgets they support; copy the ones that changed
into `budgets.json` with the reason.

## Rich lesson rendering

Chat and Learn share the UI renderer. KaTeX and Mermaid enter through dynamic imports
in `packages/ui/src/markdown/math.tsx` and `mermaid.tsx`; neither engine is needed to
parse or paint ordinary messages. KaTeX emits native MathML, so it ships no font files
or vendor stylesheet. Mermaid uses the maintained 11.x line: 12.x adds an ELK dependency
outside the repository's licence allowlist. A scoped npm override makes Mermaid
share the patched direct KaTeX 0.19 engine, avoiding both its vulnerable 0.16 dependency
and a second math runtime.

The lazy vendor cost lands in the total JavaScript, total distribution, gzipped JavaScript and
largest-chunk budgets: Mermaid's diagram chunks take web/dist from about 0.8 MB to 4.2 MB.
The initial load grew by 5 KB (1.2 KB gzipped) for the shared renderer and the Learn fixes.
The service worker still precaches the whole distribution for offline use; lazy imports
keep vendor parsing/execution out of the initial page, not out of that background cache.

`packages/ui/src/markdown/lesson.perf.test.tsx` mounts 200 messages exceeding 200 KB,
including tables, highlighted code and 400 math expressions, within five seconds.
Additional 200 KB cases guard the highlighter against quadratic scans of unterminated
strings/comments; the original parser's adversarial timing cases remain in its moved suite.
