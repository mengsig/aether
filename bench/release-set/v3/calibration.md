# Release-set calibration

- Candidates: 24; trials: 96; arms: haiku/new, luna/new; repeats: 2
- Priced spend: $2.53 API-equivalent

Eligible: no infra error, at least 2 counted repeats per model, and a new Haiku pass rate from 0% to 100%.

| Template | Candidates | Haiku pass | Luna pass | Eligible | Chosen |
|---|---:|---:|---:|---:|---:|
| bundle | 24 | 38% | 38% | 24 | 20 |

Chosen: **20 tasks**, mean new Haiku pass rate **45%**, mean new Luna pass rate **45%**; a release run on 10 workers takes at least 79 min.

## Acceptance: **accepted**

- Targets met: at least 20 tasks, Haiku mean within 20% of 50%, Luna mean inside 30%-70%, a release run within 90 min on 10 workers.
