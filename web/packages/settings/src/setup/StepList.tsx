// The steps of a setup run as a calm vertical list: what is happening now, what is done, and on the
// step that stopped, the plain reason with the one or two actions that fix it.

import { useApp } from "@aether-ui/core";
import { Button } from "@aether-ui/ui";
import { useState } from "react";
import { invariant } from "../kit/invariant.ts";
import { INSTALL_GUIDE, type SetupError, type SetupFacts, type StepId } from "./steps.ts";
import type { SetupRun, StepState } from "./useSetup.ts";

const TITLE: Readonly<Record<StepId, string>> = {
  tools: "Find Aether and Archon",
  connect: "Connect Archon's tools to Aether",
  start: "Start Aether and Archon",
  check: "Check that everything works",
};

const RUNNING: Readonly<Record<StepId, string>> = {
  tools: "Looking for Aether and Archon on this computer…",
  connect: "Connecting Archon's tools…",
  start: "Starting Aether and Archon…",
  check: "Checking Archon's tools and your sign-ins…",
};

const NAME = { aether: "Aether", archon: "Archon" } as const;

/** What a finished step found, in one short line. */
export function doneText(step: StepId, facts: SetupFacts): string {
  switch (step) {
    case "tools": return (facts.tools ?? []).map((t) => `${NAME[t.name]}${t.version ? ` ${t.version}` : ""}`).join(" and ") + " found";
    case "connect":
      if (facts.registration === "dedicated") return "Archon uses its own private Aether, as Archon's settings say";
      return facts.registration === "already_present" ? "Archon's tools were already connected" : "Archon's tools are connected";
    case "start": return facts.restarted ? "Aether restarted and Archon is running" : "Aether and Archon are running";
    case "check": {
      const ready = (facts.providers ?? []).filter((p) => p.readiness.ready).map((p) => p.name);
      return `${facts.archonTools ?? 0} Archon tools ready · runs work with ${ready.join(", ")}`;
    }
  }
}

function Marker({ state, index }: { readonly state: StepState["state"]; readonly index: number }) {
  if (state === "done") return <span className="su-marker" data-state="done" aria-hidden="true">✓</span>;
  if (state === "failed") return <span className="su-marker" data-state="failed" aria-hidden="true">!</span>;
  if (state === "running") return <span className="su-marker" data-state="running" aria-hidden="true"><span className="su-spinner" /></span>;
  return <span className="su-marker" data-state="waiting" aria-hidden="true">{index + 1}</span>;
}

function localAether(app: ReturnType<typeof useApp>): string | null {
  const local = app.manager.all().find((c) => c.endpoint.daemon === "aether" && c.endpoint.transport.id === "ipc" && c.current.state === "ready");
  return local?.id ?? null;
}

function Fixes({ step, error, run }: { readonly step: StepId; readonly error: SetupError; readonly run: SetupRun }) {
  const app = useApp();
  const [copied, setCopied] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const desktop = app.host.desktop;
  const copy = async (name: keyof typeof INSTALL_GUIDE) => {
    try {
      await navigator.clipboard.writeText(INSTALL_GUIDE[name].command);
      setCopied(name);
    } catch (failure) {
      setNote(`Could not copy the command: ${failure instanceof Error ? failure.message : String(failure)}`);
    }
  };
  const signIn = async (engine: "claude" | "codex") => {
    const conn = localAether(app);
    if (conn === null) return setNote("Aether is not answering yet. Try again in a moment.");
    try {
      await app.privileged(conn, { kind: "engine.login", engine });
      setNote("Finish signing in in the window that opened, then check again.");
    } catch (failure) {
      setNote(`Sign-in did not open: ${failure instanceof Error ? failure.message : String(failure)}`);
    }
  };
  return (
    <div className="su-fixes">
      {error.missing.map((name) => (
        <span key={name} className="su-fix-group">
          {desktop ? <Button size="sm" onClick={() => void desktop.openExternal(INSTALL_GUIDE[name].url)}>How to install {NAME[name]}</Button> : null}
          <Button size="sm" variant="quiet" onClick={() => void copy(name)}>{copied === name ? "Install command copied" : `Copy the ${NAME[name]} install command`}</Button>
        </span>
      ))}
      {error.signIn.map((engine) => (
        <Button key={engine} size="sm" onClick={() => void signIn(engine)}>Sign in to {engine === "claude" ? "Claude" : "Codex"}</Button>
      ))}
      <Button size="sm" variant="primary" disabled={run.busy} onClick={() => void run.run(error.retryFrom ?? step)}>
        {error.missing.length > 0 || error.signIn.length > 0 ? "Check again" : "Try again"}
      </Button>
      {note === null ? null : <p className="su-fix-note" role="status">{note}</p>}
    </div>
  );
}

export function StepList({ run }: { readonly run: SetupRun }) {
  invariant(run.steps.length > 0, "a step list shows steps");
  return (
    <ol className="su-steps" aria-label="Setup steps">
      {run.steps.map((step, index) => {
        const state = run.states[step];
        return (
          <li key={step} className="su-step" data-state={state.state} aria-current={state.state === "running" ? "step" : undefined}>
            <Marker state={state.state} index={index} />
            <div className="su-step-body">
              <h3 className="su-step-title">{TITLE[step]}</h3>
              {state.state === "running" ? <p className="su-step-detail" role="status">{RUNNING[step]}</p> : null}
              {state.state === "done" ? <p className="su-step-detail">{doneText(step, run.facts)}</p> : null}
              {state.state === "failed" ? (
                <>
                  <p className="su-step-detail" role="alert" data-state="failed">{state.error.message}</p>
                  <Fixes step={step} error={state.error} run={run} />
                </>
              ) : null}
            </div>
          </li>
        );
      })}
    </ol>
  );
}
