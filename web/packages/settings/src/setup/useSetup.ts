// Runs setup steps one after another and keeps how each went, so a screen shows progress, a plain
// error on the step that stopped, and a retry that picks up from there.

import { useApp } from "@aether-ui/core";
import type { DesktopSetup } from "@aether-ui/protocol";
import { useCallback, useRef, useState } from "react";
import { invariant } from "../kit/invariant.ts";
import { runStep, SetupError, STEP_ORDER, type SetupFacts, type StepId } from "./steps.ts";

export type StepState =
  | { readonly state: "waiting" | "running" | "done" }
  | { readonly state: "failed"; readonly error: SetupError };

export interface SetupRun {
  readonly steps: readonly StepId[];
  readonly states: Readonly<Record<StepId, StepState>>;
  readonly facts: SetupFacts;
  readonly busy: boolean;
  readonly finished: boolean;
  /** Runs every step from `from` (else the first one not done); a no-op while a run is going. */
  readonly run: (from?: StepId) => Promise<void>;
}

const WAITING: StepState = { state: "waiting" };

function initialStates(): Record<StepId, StepState> {
  return { tools: WAITING, connect: WAITING, start: WAITING, check: WAITING };
}

function asSetupError(error: unknown): SetupError {
  if (error instanceof SetupError) return error;
  const message = error instanceof Error ? error.message : String(error);
  return new SetupError(message || "This step did not finish.");
}

export function useSetup(setup: DesktopSetup, steps: readonly StepId[] = STEP_ORDER): SetupRun {
  invariant(steps.length > 0, "a setup run has steps");
  invariant(steps.every((step, i) => i === 0 || STEP_ORDER.indexOf(step) > STEP_ORDER.indexOf(steps[i - 1] ?? "tools")), "steps keep the setup order");
  const app = useApp();
  const [states, setStates] = useState<Record<StepId, StepState>>(initialStates);
  const [facts, setFacts] = useState<SetupFacts>({});
  const [busy, setBusy] = useState(false);
  const running = useRef(false);
  const known = useRef<{ states: Record<StepId, StepState>; facts: SetupFacts }>({ states: initialStates(), facts: {} });

  const mark = useCallback((step: StepId, state: StepState) => {
    known.current = { ...known.current, states: { ...known.current.states, [step]: state } };
    setStates(known.current.states);
  }, []);

  const run = useCallback(async (from?: StepId) => {
    if (running.current) return;
    invariant(from === undefined || steps.includes(from), `${String(from)} is a step of this run`);
    running.current = true;
    if (from !== undefined) for (const step of steps.slice(steps.indexOf(from))) mark(step, WAITING);
    setBusy(true);
    try {
      for (const step of steps) {
        if (known.current.states[step].state === "done") continue;
        mark(step, { state: "running" });
        try {
          const learned = await runStep(step, known.current.facts, { app, setup });
          known.current = { ...known.current, facts: { ...known.current.facts, ...learned } };
          setFacts(known.current.facts);
          mark(step, { state: "done" });
        } catch (error) {
          mark(step, { state: "failed", error: asSetupError(error) });
          return;
        }
      }
    } finally {
      running.current = false;
      setBusy(false);
    }
  }, [app, mark, setup, steps]);

  const finished = steps.every((step) => states[step].state === "done");
  return { steps, states, facts, busy, finished, run };
}
