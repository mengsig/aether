# Release bench: INCONCLUSIVE, both tiers PASS (A/A check of set v2, Aether 1.1.0 against itself, 2026-10-18)

Produced by `make bench-release` with the same binary as `OLD` and `NEW`, and
`ALLOW_OFF_TARGET=1` because set v2 missed its Haiku target; see
[release-bench.md](release-bench.md#aa-check-of-v2). The overall verdict is
INCONCLUSIVE by design on an off-target set. The gate itself passed on both tiers.

The run went in two legs under one run id. The first leg ran 461 trials at
`BUDGET=14` and was interrupted once budget headroom left a single trial
running. The second resumed at the default `BUDGET=20` and ran the other 199.
The interrupted attempt was moved aside and charged its reservation. The
header's wall time covers the second leg only; the whole run took 95 minutes.


- Task set: `bench/release-set/v2` (110 tasks, sha256 `7f3166c96e35e2ad`) **(off target: not a release verdict)**
- Old: `6350ccf0072061b6`, candidate: `6350ccf0072061b6` (same binary: an A/A run)
- Native CLIs: claude 2.1.299 (Claude Code), codex codex-cli 0.163.0
- Arms: haiku/native, haiku/old, haiku/new, luna/native, luna/old, luna/new; repeats 1; jobs 10; timeout 600s
- Trials: 660; priced spend $8.90 API-equivalent
- Wall time this invocation: 30.8 min

## Arms

| Arm | Trials / planned | Infra errors | Harness failures | Pass rate [95% CI] | Check fraction | Cost / trial | Total cost | Median time |
|---|---:|---:|---:|---|---:|---:|---:|---:|
| haiku/native | 110 / 110 | 0 | 0 | 95% [91%, 99%] | 99% | $0.020 | $2.217 | 48s |
| haiku/new | 110 / 110 | 0 | 0 | 95% [91%, 99%] | 99% | $0.021 | $2.318 | 48s |
| haiku/old | 110 / 110 | 0 | 0 | 95% [90%, 98%] | 99% | $0.021 | $2.331 | 49s |
| luna/native | 110 / 110 | 0 | 0 | 35% [25%, 44%] | 74% | $0.005 | $0.569 | 23s |
| luna/new | 110 / 110 | 0 | 0 | 44% [35%, 53%] | 84% | $0.006 | $0.680 (1 unpriced) | 29s |
| luna/old | 110 / 110 | 0 | 0 | 54% [45%, 63%] | 87% | $0.007 | $0.784 (1 unpriced) | 33s |

## Release gate

### haiku: **PASS**

- **candidate vs old** (haiku/new vs haiku/old, 110 tasks, 110 pairs): pass +0.9% [-2.7%, +4.5%], check fraction +0.4% [-0.3%, +1.2%], cost x0.99 [x0.85, x1.17]; McNemar old-only 3 / new-only 4, p = 1.000
- Infra error rate: haiku/old 0.0%, haiku/new 0.0%

### luna: **PASS**

- **candidate vs old** (luna/new vs luna/old, 110 tasks, 110 pairs): pass -10.0% [-19.1%, -0.9%], check fraction -2.8% [-6.8%, +1.1%], cost x0.84 [x0.75, x0.95]; McNemar old-only 24 / new-only 13, p = 0.099
- Infra error rate: luna/old 0.0%, luna/new 0.0%
- Warning: pass rate point estimate -10.0% is below -5%

## Drift reference (Aether old release vs the native CLI)

- **haiku** (haiku/old vs haiku/native, 110 tasks, 110 pairs): pass -0.9% [-4.5%, +2.7%], check fraction -0.4% [-1.3%, +0.3%], cost x1.05 [x0.82, x1.35]; McNemar old-only 3 / new-only 2, p = 1.000
- **luna** (luna/old vs luna/native, 110 tasks, 110 pairs): pass +19.1% [+10.0%, +29.1%], check fraction +12.1% [+6.4%, +18.1%], cost x1.39 [x1.10, x1.76]; McNemar old-only 5 / new-only 26, p = 0.000

A large change here between two reports means the models or native CLIs moved, not Aether.

## Pass rate by family

| Family | haiku/native | haiku/new | haiku/old | luna/native | luna/new | luna/old |
|---|---:|---:|---:|---:|---:|---:|
| algo | 100% | 100% | 100% | 75% | 100% | 100% |
| delegation | 100% | 100% | 100% | 100% | 100% | 100% |
| docspec | 95% | 95% | 95% | 21% | 36% | 36% |
| edgecase | 100% | 100% | 95% | 48% | 43% | 52% |
| longctx | 100% | 100% | 100% | 0% | 0% | 14% |
| multibug | 100% | 100% | 100% | 100% | 100% | 100% |
| port | 100% | 100% | 100% | 9% | 27% | 45% |
| refactor | 62% | 62% | 62% | 12% | 38% | 75% |
| traceback | 100% | 100% | 100% | 60% | 40% | 100% |
