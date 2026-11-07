// The app's long-lived setup runs: first-run setup and the Archon tools repair. Each belongs to the
// app, never to a view, so it survives remounts, connection switches, drops and navigation.

import { useApp, type App } from "@aether-ui/core";
import type { ConnId } from "@aether-ui/protocol";
import { useMemo, useSyncExternalStore } from "react";
import { invariant } from "../kit/invariant.ts";
import { desktopSetup, STEP_ORDER, type StepId } from "./steps.ts";
import { SetupRunner, type SetupOutcome, type SetupRun } from "./useSetup.ts";

export const REPAIR_STEPS: readonly StepId[] = ["connect", "start", "check"];

export interface SetupRuns {
  /** "Set up on this computer": every step. */
  readonly setup: SetupRunner;
  /** "Connect Archon tools": the steps that register Archon's tools and check them. */
  readonly repair: SetupRunner;
}

const OUTCOME_TEXT = {
  setup: { ok: "This computer is set up", stopped: "Setup stopped" },
  repair: { ok: "Archon's tools are connected and ready", stopped: "Connecting Archon's tools stopped" },
} as const;

const owned = new WeakMap<App, SetupRuns>();

/** Each run says how it ended exactly once, whichever views are showing it by then. */
function reporter(app: App, kind: keyof typeof OUTCOME_TEXT): (outcome: SetupOutcome) => void {
  return (outcome) => {
    if (outcome.ok) return app.store.report(OUTCOME_TEXT[kind].ok, undefined, undefined, "success");
    invariant(outcome.error.message.trim() !== "", "a stopped run says why");
    app.store.report(`${OUTCOME_TEXT[kind].stopped}: ${outcome.error.message}`, undefined, undefined, "danger");
  };
}

/** The app's runs, made on first use; null where this app cannot set up the computer. */
export function setupRunsOf(app: App): SetupRuns | null {
  const known = owned.get(app);
  if (known !== undefined) return known;
  const setup = desktopSetup(app);
  if (setup === null) return null;
  const runs: SetupRuns = {
    setup: new SetupRunner(app, setup, STEP_ORDER, reporter(app, "setup")),
    repair: new SetupRunner(app, setup, REPAIR_STEPS, reporter(app, "repair")),
  };
  owned.set(app, runs);
  return runs;
}

export function useSetupRuns(): SetupRuns | null {
  const app = useApp();
  return useMemo(() => setupRunsOf(app), [app]);
}

const NO_RUN = (): null => null;
const NO_SUBSCRIPTION = (): (() => void) => () => undefined;

/** The connection a running repair started from, which Settings keeps showing until it ends. */
export function useRepairPin(): ConnId | null {
  const repair = useSetupRuns()?.repair;
  const run: SetupRun | null = useSyncExternalStore(repair?.subscribe ?? NO_SUBSCRIPTION, repair?.get ?? NO_RUN);
  return run?.busy === true ? run.origin : null;
}
