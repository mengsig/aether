# Desktop journeys

End-to-end journeys for the packaged desktop app (`target/desktop/linux-unpacked/aether-desktop`).
Each spec file gets a fresh private stack: this branch's aetherd and archond in a temp root, a
headless sway, and one packaged-app instance on the stack's sockets. Everything is stopped and every
temp root removed after the file, and the runner checks that afterwards.

| Journey | Spec | What it walks through |
| --- | --- | --- |
| 1 first launch | `first-launch.spec.ts` | Smoke: the window connects, the fleet renders, Start-agent shows. Then: start an agent from Activity, see the reply and its shell call, message it, stop and resume. Records `first-launch.webm`. |
| 2 task for an existing agent | `task-for-existing-agent.spec.ts` | Task mode in the start sheet, then an owner: the task lands on that agent. |
| 3 approvals | `approvals.spec.ts` | A tool call waits for approval, Inbox approves it, and the agent goes on. Then a rule is added from Controls. |
| 4 schedules | `schedules.spec.ts` | Create a weekday cron schedule that starts a new agent, see it listed, delete it. |
| 5 memory | `memory.spec.ts` | Create a global memory, then find it by search. |
| 6 archon | `archon.spec.ts` | Register a scratch repo, queue an item on it, wait (bounded) for an outcome, then open the task graph and history. |
| 7 learn | `learn.spec.ts` | Turn Noesis on (main confirms natively), start a session, answer the Prober, get a practice problem. |
| 8 drafts | `drafts.spec.ts` | An objective typed in the start sheet survives leaving Activity and coming back. |

## Running

Faux engine (the default, for CI; free and hermetic):

```sh
scripts/e2e_journeys.sh
scripts/e2e_journeys.sh first-launch     # one file; arguments go to `playwright test`
```

Real engines, on demand, through your own `claude` and `codex` logins:

```sh
JOURNEY_ENGINE=real scripts/e2e_journeys.sh first-launch
```

The script builds nothing it can reuse, and it never runs cargo:

- `AETHER_BIN` and `ARCHOND_BIN` come from the environment. Unset, `AETHER_BIN` is
  `target/debug/aether`, and `ARCHOND_BIN` is the newest `target/e2e-archon/archond-<sha>` that
  `scripts/e2e.sh` cached and that was built from the vendored `web/schemas` copy. If either is
  missing, run `scripts/e2e.sh` once or set the variable.
- The app comes from `E2E_DESKTOP_EXEC`. Unset, it is `target/desktop/linux-unpacked/aether-desktop`.
  If that is missing, or `E2E_REPACKAGE=1`, the script packages it under `scripts/ci_slot.sh`
  (`node build/package.ts --target linux --dir`) from the committed `web/dist`. That needs more
  than 10 GB free.
- Playwright runs under the CI slot with a private HOME and XDG dirs. The script then runs
  `scripts/e2e.sh`'s leftover check over every daemon, sway and app process the harness started.

To run the config directly (this skips the leftover check, and the binaries must already be in
the environment):

```sh
npx playwright test -c web/e2e/specs/journeys/playwright.config.ts
```

Reports go to `$E2E_OUT/journeys/` (default `web/e2e/.out/journeys/`): `report.json` and, per
test, `results/` with the first-launch video.

The base suite (`web/e2e/playwright.config.ts`) also lists these specs, but skips them there. They
only run through the journeys config.

## How the app is driven

- **CDP, not `_electron`.** The packaged app's fuses turn off Node's inspector flags, so
  Playwright's `_electron.launch` cannot attach. `harness/packaged.ts` starts the binary with
  `--remote-debugging-port=0` on loopback, reads the DevTools endpoint from its log, and connects
  with `chromium.connectOverCDP`. Only one app instance runs at a time.
- **Headless sway.** `sway -c /dev/null` runs with `WLR_BACKENDS=headless` and its own
  `XDG_RUNTIME_DIR`. The app runs on it with `--ozone-platform=wayland`. Native confirmations
  (Noesis settings) are answered with `swaymsg` focus and `wtype`. This needs `sway`, `swaymsg`
  and `wtype` on PATH.
- **Sockets.** The app gets `AETHER_SOCKET` and `ARCHON_SOCKET` for the stack. Local sockets need
  no pairing, so the smoke records that as a fallback. If a later UI asks to pair, the journey
  pairs with a code from the host, as `harness/pair.ts` does.
- **The faux engine.** `harness/engine.ts` scripts the replies (`specs/journeys/scripts.ts`): a
  `bash` call for journeys 1 and 3, and the Prober's and Practitioner's tool calls for journey 7.

## The real variant

`JOURNEY_ENGINE=real` switches each stack's aetherd to `claude-code` and `codex` and restarts it
on the same root:

- **CLIs.** The two CLIs are found on your PATH and linked into the stack root. They read their
  logins in place: `CLAUDE_CONFIG_DIR` defaults to `~/.claude` and `CODEX_HOME` to `~/.codex`.
  Override them with `E2E_CLAUDE_CONFIG_DIR` and `E2E_CODEX_HOME`, and the search path with
  `E2E_USER_PATH`. Nothing is copied, and daemon state stays in the temp root. The CLIs may write
  their own caches in their login dirs.
- **Readiness.** `list_models` must show both `claude-code/claude-haiku-5-5` and
  `codex/gpt-6-luna` as configured within two minutes. Otherwise every journey skips and says
  which model is missing and how to log in.
- **Spend.** Only those two models run.
  - The daemon config caps each run: `run_hard_stop_usd = 0.50` and `run_budget_usd = 1.00`,
    with one engine turn at a time.
  - Every agent a journey starts or creates gets `cost_budget = 0.25` and `max_turns = 4`.
  - Objectives are one shell call and one word.
  - Journey 1 records the agent's cost fields as a `cost` annotation.
  - Journey 7 is faux-only, and schedules and owners use the faux model, which is free.
  - archond's orchestrator stays on the faux engine.

## Skips: waiting for the new UI

Each step looks up the new UI's `data-testid` first, then today's control by role or label.

- **A fallback was used:** the test gets a `fallback` annotation.
- **No control at all:** the step is recorded as `missing`.
  - If later steps depend on it, the test skips there, with the reason "waits for the new UI:
    [data-testid=…] is missing".
  - If not, the steps after it run, and the test skips at the end, naming every missing control.

So a skipped journey says exactly which test ids it needs, and it passes without change once
they land. Every wait is bounded.
