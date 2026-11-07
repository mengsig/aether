// Runs setup steps one after another and keeps how each went, so a screen shows progress, a plain
// error on the step that stopped, and a retry that picks up from there.

import type { App } from "@aether-ui/core";
import type { ConnId, DesktopSetup } from "@aether-ui/protocol";
import { useSyncExternalStore } from "react";
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
  /** The connection the view that started the run was showing, kept across its retries. */
  readonly origin: ConnId | null;
  /** Runs every step from `from` (else the first one not done); a no-op while a run is going. */
  readonly run: (from?: StepId, origin?: ConnId) => Promise<void>;
}

/** How one run ended: every step done, or the step that stopped it and why. */
export type SetupOutcome =
  | { readonly ok: true }
  | { readonly ok: false; readonly step: StepId; readonly error: SetupError };

const WAITING: StepState = { state: "waiting" };

function initialStates(): Record<StepId, StepState> {
  return { tools: WAITING, connect: WAITING, start: WAITING, check: WAITING };
}

function asSetupError(error: unknown): SetupError {
  if (error instanceof SetupError) return error;
  const message = error instanceof Error ? error.message : String(error);
  return new SetupError(message || "This step did not finish.");
}

/**
 * One setup run held outside React, so a run outlives the view that started it and any view that
 * mounts later shows the same progress. `onOutcome` hears how each run ended, once per run.
 */
export class SetupRunner {
  readonly steps: readonly StepId[];
  private readonly app: App;
  private readonly setup: DesktopSetup;
  private readonly onOutcome: ((outcome: SetupOutcome) => void) | undefined;
  private readonly listeners = new Set<() => void>();
  private readonly finishListeners = new Set<() => void>();
  private snapshot: SetupRun;

  constructor(app: App, setup: DesktopSetup, steps: readonly StepId[] = STEP_ORDER, onOutcome?: (outcome: SetupOutcome) => void) {
    invariant(steps.length > 0, "a setup run has steps");
    invariant(steps.every((step, i) => i === 0 || STEP_ORDER.indexOf(step) > STEP_ORDER.indexOf(steps[i - 1] ?? "tools")), "steps keep the setup order");
    this.app = app;
    this.setup = setup;
    this.steps = steps;
    this.onOutcome = onOutcome;
    this.snapshot = { steps, states: initialStates(), facts: {}, busy: false, finished: false, origin: null, run: this.run };
  }

  readonly get = (): SetupRun => this.snapshot;

  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };

  /** Calls `listener` each time a run completes every step, for views that reread then. */
  readonly whenFinished = (listener: () => void): (() => void) => {
    this.finishListeners.add(listener);
    return () => { this.finishListeners.delete(listener); };
  };

  readonly run = async (from?: StepId, origin?: ConnId): Promise<void> => {
    if (this.snapshot.busy) return;
    invariant(from === undefined || this.steps.includes(from), `${String(from)} is a step of this run`);
    if (from !== undefined) for (const step of this.steps.slice(this.steps.indexOf(from))) this.mark(step, WAITING);
    this.update({ busy: true, origin: origin ?? this.snapshot.origin });
    let outcome: SetupOutcome;
    try {
      outcome = await this.runSteps();
    } finally {
      this.update({ busy: false });
    }
    invariant(outcome.ok === this.snapshot.finished, "a run ends finished exactly when no step stopped it");
    this.onOutcome?.(outcome);
    if (outcome.ok) for (const listener of [...this.finishListeners]) listener();
  };

  private async runSteps(): Promise<SetupOutcome> {
    for (const step of this.steps) {
      if (this.snapshot.states[step].state === "done") continue;
      this.mark(step, { state: "running" });
      try {
        const learned = await runStep(step, this.snapshot.facts, { app: this.app, setup: this.setup });
        this.update({ facts: { ...this.snapshot.facts, ...learned } });
        this.mark(step, { state: "done" });
      } catch (error) {
        const failure = asSetupError(error);
        this.mark(step, { state: "failed", error: failure });
        return { ok: false, step, error: failure };
      }
    }
    return { ok: true };
  }

  private mark(step: StepId, state: StepState): void {
    this.update({ states: { ...this.snapshot.states, [step]: state } });
  }

  private update(change: Partial<Pick<SetupRun, "states" | "facts" | "busy" | "origin">>): void {
    const next = { ...this.snapshot, ...change };
    this.snapshot = { ...next, finished: this.steps.every((step) => next.states[step].state === "done") };
    for (const listener of this.listeners) listener();
  }
}

/** Follows a runner's progress in a view. */
export function useSetupRunner(runner: SetupRunner): SetupRun {
  return useSyncExternalStore(runner.subscribe, runner.get);
}
