// The overview at #/: on a computer with nothing set up, one clear way to set it up (and pairing as
// the second choice); once connected, the next thing to do.

import { selectConnections, useApp, useCapabilities, useSelector } from "@aether-ui/core";
import { useMemo } from "react";
import { invariant } from "../kit/invariant.ts";
import { desktopSetup } from "./steps.ts";
import "./setup.css";

export function Welcome() {
  const app = useApp();
  const canSetUp = useMemo(() => desktopSetup(app) !== null, [app]);
  const caps = useCapabilities();
  const connections = Object.values(useSelector(selectConnections));
  const aether = connections.filter((c) => c.daemon === "aether");
  if (aether.some((c) => c.state === "ready")) return <Connected archon={connections.some((c) => c.daemon === "archon" && c.state === "ready")} />;
  if (aether.length > 0) return <Waiting text="Connecting to Aether…" canSetUp={canSetUp} />;
  if (caps["desktop.managed_start"] === true) return <Waiting text="Starting Aether and Archon on this computer…" canSetUp={canSetUp} />;
  return <FirstRun canSetUp={canSetUp} />;
}

function FirstRun({ canSetUp }: { readonly canSetUp: boolean }) {
  return (
    <div className="ui-page su-page">
      <section className="su-card su-welcome" aria-labelledby="su-welcome-title">
        <header className="su-hero">
          <p className="su-eyebrow">Welcome</p>
          <h1 id="su-welcome-title" className="su-title">Run AI agents on your own computer</h1>
          <p className="su-lede">
            {canSetUp
              ? "Set up Aether and Archon here in one step, or connect to a computer that already runs them."
              : "Connect this browser to a computer that runs Aether."}
          </p>
        </header>
        <div className="su-choices">
          {canSetUp ? (
            <a className="su-choice" data-primary="true" href="#/setup">
              <span className="su-choice-title">Set up on this computer</span>
              <span className="su-choice-text">Finds Aether and Archon, connects Archon's tools, starts both and checks them.</span>
            </a>
          ) : null}
          <a className="su-choice" href="#/pair/aether">
            <span className="su-choice-title">Pair with another computer</span>
            <span className="su-choice-text">Use Aether running somewhere else, with a code from that computer.</span>
          </a>
        </div>
      </section>
    </div>
  );
}

function Waiting({ text, canSetUp }: { readonly text: string; readonly canSetUp: boolean }) {
  invariant(text.trim() !== "", "a wait says what it waits for");
  return (
    <div className="ui-page su-page">
      <section className="su-card su-welcome" aria-busy="true">
        <p className="su-waiting" role="status"><span className="su-spinner" aria-hidden="true" />{text}</p>
        {canSetUp ? <a className="ui-link" href="#/setup">Run setup again</a> : null}
      </section>
    </div>
  );
}

function Connected({ archon }: { readonly archon: boolean }) {
  return (
    <div className="ui-page su-page">
      <section className="su-card su-welcome" aria-labelledby="su-home-title">
        <header className="su-hero">
          <p className="su-eyebrow">Ready</p>
          <h1 id="su-home-title" className="su-title">What should an agent do next?</h1>
          <p className="su-lede">Start a task and follow it as it runs.</p>
        </header>
        <footer className="su-actions">
          <a className="ui-button su-primary" data-variant="primary" data-size="md" href="#/activity/start">Start a task</a>
          <a className="ui-link" href="#/activity">See recent activity</a>
          {archon ? <a className="ui-link" href="#/archon/queue">Open Archon's queue</a> : null}
        </footer>
      </section>
    </div>
  );
}
