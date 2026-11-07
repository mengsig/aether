// "Set up on this computer": one button runs every step, each step shows its progress and, if it
// stops, why and how to fix it; the run ends on a confirmation with the next thing to do.

import { useApp } from "@aether-ui/core";
import { Button, EmptyState } from "@aether-ui/ui";
import type { DesktopSetup } from "@aether-ui/protocol";
import { useEffect, useMemo, useRef } from "react";
import { invariant } from "../kit/invariant.ts";
import { desktopSetup } from "./steps.ts";
import { doneText, StepList } from "./StepList.tsx";
import { useSetup, type SetupRun } from "./useSetup.ts";
import "./setup.css";

export default function SetupScreen() {
  const app = useApp();
  const setup = useMemo(() => desktopSetup(app), [app]);
  if (setup === null) {
    return (
      <div className="ui-page su-page">
        <EmptyState title="Setup runs in the desktop app" action={<a className="ui-link" href="#/pair/aether">Pair with a computer instead</a>}>
          Open the Aether desktop app on the computer you want to use, and set it up from there.
        </EmptyState>
      </div>
    );
  }
  return <SetupFlow setup={setup} />;
}

function SetupFlow({ setup }: { readonly setup: DesktopSetup }) {
  const run = useSetup(setup);
  invariant(run.steps.includes("check"), "first-run setup ends by checking its work");
  if (run.finished) return <Ready run={run} />;
  const started = run.steps.some((step) => run.states[step].state !== "waiting");
  return (
    <div className="ui-page su-page">
      <section className="su-card" aria-labelledby="su-title">
        <header className="su-hero">
          <p className="su-eyebrow">This computer</p>
          <h1 id="su-title" className="su-title">Set up Aether and Archon</h1>
          <p className="su-lede">One click finds both apps, connects Archon's tools to Aether, starts them, and checks that work can run.</p>
        </header>
        <StepList run={run} />
        {started ? null : (
          <footer className="su-actions">
            <Button variant="primary" className="su-primary" data-autofocus onClick={() => void run.run()}>Set up this computer</Button>
            <a className="ui-link" href="#/pair/aether">Pair with another computer instead</a>
          </footer>
        )}
      </section>
    </div>
  );
}

function Ready({ run }: { readonly run: SetupRun }) {
  invariant(run.finished, "the confirmation follows a finished run");
  const next = useRef<HTMLAnchorElement>(null);
  useEffect(() => next.current?.focus(), []);
  return (
    <div className="ui-page su-page">
      <section className="su-card su-ready" aria-labelledby="su-ready-title">
        <span className="su-badge" aria-hidden="true">✓</span>
        <h1 id="su-ready-title" className="su-title">This computer is ready</h1>
        <ul className="su-summary">
          {run.steps.map((step) => <li key={step}>{doneText(step, run.facts)}</li>)}
        </ul>
        <footer className="su-actions">
          <a ref={next} className="ui-button su-primary" data-variant="primary" data-size="md" href="#/activity/start">Start your first task</a>
          <a className="ui-link" href="#/settings/providers">Review providers and tools</a>
        </footer>
      </section>
    </div>
  );
}
