# Release-set calibration

- Candidates: 143; trials: 572; arms: haiku/native, luna/native; repeats: 2
- Priced spend: $6.33 API-equivalent

Eligible: no infra error, at least 2 counted repeats per model, and not saturated (solved every time by both native models, or never by either).

| Template | Candidates | Haiku pass | Luna pass | Eligible | Chosen |
|---|---:|---:|---:|---:|---:|
| batching | 11 | 100% | 86% | 3 | 3 |
| billing | 11 | 100% | 82% | 4 | 4 |
| bucket | 11 | 100% | 64% | 7 | 6 |
| config | 11 | 100% | 68% | 6 | 5 |
| cron | 11 | 100% | 64% | 7 | 6 |
| diff | 11 | 100% | 77% | 5 | 5 |
| duration | 11 | 100% | 32% | 11 | 10 |
| ledger | 11 | 100% | 68% | 5 | 5 |
| pricing | 11 | 100% | 9% | 11 | 11 |
| query | 11 | 86% | 0% | 11 | 11 |
| ranges | 11 | 91% | 73% | 5 | 5 |
| slug | 11 | 100% | 41% | 10 | 10 |
| versions | 11 | 100% | 14% | 11 | 11 |

Chosen: **92 tasks**, mean native Haiku pass rate **97%**, mean native Luna pass rate **30%**.
