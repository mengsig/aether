# Release-set calibration

- Candidates: 40; trials: 160; arms: haiku/native, luna/native; repeats: 2
- Priced spend: $1.97 API-equivalent
- Carried from `bench/release-set/v1` with the rates its calibration recorded: 81 tasks

Eligible: no infra error, at least 2 counted repeats per model, and a native Haiku pass rate from 0% to 100%.

| Template | Candidates | Haiku pass | Luna pass | Eligible | Chosen |
|---|---:|---:|---:|---:|---:|
| batching | 3 | 100% | 50% | 3 | 3 |
| billing | 4 | 100% | 50% | 4 | 4 |
| bucket | 6 | 100% | 50% | 6 | 6 |
| config | 5 | 100% | 50% | 5 | 5 |
| cron | 6 | 100% | 50% | 6 | 6 |
| diff | 5 | 100% | 50% | 5 | 5 |
| duration | 10 | 100% | 35% | 10 | 10 |
| exporters | 10 | 75% | 55% | 10 | 8 |
| ledger | 5 | 100% | 30% | 5 | 5 |
| pricing | 11 | 100% | 9% | 11 | 11 |
| protocol | 10 | 100% | 30% | 10 | 7 |
| query | 11 | 86% | 0% | 11 | 11 |
| ranges | 5 | 80% | 40% | 5 | 5 |
| slug | 10 | 100% | 35% | 10 | 10 |
| versions | 10 | 100% | 0% | 10 | 10 |
| workstreams | 10 | 100% | 95% | 10 | 4 |

Chosen: **110 tasks**, mean native Haiku pass rate **95%**, mean native Luna pass rate **30%**.

## Acceptance: **NOT accepted**

- mean Haiku pass 95% is not within 10% of 50%
