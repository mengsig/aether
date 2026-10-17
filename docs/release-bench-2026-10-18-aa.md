# Release bench: PASS (A/A check of set v1, Aether 1.1.0 against itself, 2026-10-18)

Produced by `make bench-release` with the same binary as `OLD` and `NEW`; see
[release-bench.md](release-bench.md#aa-check-of-v1).

- Task set: `bench/release-set/v1` (92 tasks, sha256 `a54f5849ef268512`)
- Old: `6350ccf0072061b6`, candidate: `6350ccf0072061b6` (same binary: an A/A run)
- Native CLIs: claude 2.1.299 (Claude Code), codex codex-cli 0.163.0
- Arms: haiku/native, haiku/old, haiku/new, luna/native, luna/old, luna/new; repeats 1; jobs 6; timeout 600s
- Trials: 552; priced spend $6.90 API-equivalent
- Wall time this invocation: 100.8 min

## Arms

| Arm | Trials | Infra errors | Pass rate [95% CI] | Check fraction | Cost / trial | Total cost | Median time |
|---|---:|---:|---|---:|---:|---:|---:|
| haiku/native | 92 | 0 | 97% [92%, 100%] | 99% | $0.018 | $1.668 (1 unpriced) | 49s |
| haiku/new | 92 | 0 | 97% [92%, 100%] | 99% | $0.022 | $1.980 | 48s |
| haiku/old | 92 | 0 | 97% [92%, 100%] | 99% | $0.018 | $1.629 | 45s |
| luna/native | 92 | 0 | 45% [35%, 54%] | 80% | $0.004 | $0.389 | 25s |
| luna/new | 92 | 0 | 49% [39%, 60%] | 85% | $0.007 | $0.629 | 35s |
| luna/old | 92 | 0 | 47% [37%, 58%] | 84% | $0.007 | $0.610 | 36s |

## Release gate

### haiku: **PASS**

- **candidate vs old** (haiku/new vs haiku/old, 92 tasks, 92 pairs): pass +0.0% [-3.3%, +3.3%], check fraction +0.3% [-0.7%, +1.7%], cost x1.22 [x1.05, x1.41]; McNemar old-only 2 / new-only 2, p = 1.000
- Infra error rate (old + candidate): 0.0%
- Warning: cost point estimate x1.22 is above x1.15

### luna: **PASS**

- **candidate vs old** (luna/new vs luna/old, 92 tasks, 92 pairs): pass +2.2% [-8.7%, +12.0%], check fraction +0.4% [-3.9%, +5.0%], cost x1.03 [x0.87, x1.22]; McNemar old-only 16 / new-only 18, p = 0.864
- Infra error rate (old + candidate): 0.0%

## Drift reference (Aether old release vs the native CLI)

- **haiku** (haiku/old vs haiku/native, 92 tasks, 92 pairs): pass +0.0% [-5.4%, +5.4%], check fraction +0.1% [-2.0%, +2.1%], cost x0.96 [x0.91, x1.01]; McNemar old-only 3 / new-only 3, p = 1.000
- **luna** (luna/old vs luna/native, 92 tasks, 92 pairs): pass +2.2% [-9.8%, +14.1%], check fraction +3.9% [-1.4%, +9.2%], cost x1.57 [x1.32, x1.88]; McNemar old-only 15 / new-only 17, p = 0.860

A large change here between two reports means the models or native CLIs moved, not Aether.

## Pass rate by family

| Family | haiku/native | haiku/new | haiku/old | luna/native | luna/new | luna/old |
|---|---:|---:|---:|---:|---:|---:|
| algo | 100% | 100% | 100% | 62% | 100% | 75% |
| docspec | 93% | 93% | 93% | 30% | 37% | 28% |
| edgecase | 100% | 100% | 100% | 57% | 52% | 67% |
| multibug | 100% | 100% | 100% | 100% | 75% | 100% |
| port | 100% | 100% | 100% | 36% | 18% | 36% |
| traceback | 100% | 100% | 100% | 60% | 100% | 60% |
