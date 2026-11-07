// The overview at #/: on a computer with nothing set up, one clear way to set it up (and pairing as
// the second choice); once connected, the next thing to do.

import { selectConnections, useApp, useCapabilities, useReread, useSelector } from "@aether-ui/core";
import type { ConnId } from "@aether-ui/protocol";
import { Button } from "@aether-ui/ui";
import { useMemo } from "react";
import { invariant } from "../kit/invariant.ts";
import { ConnectArchon } from "./ConnectArchon.tsx";
import { archonTools, mcpServersOf } from "./readiness.ts";
import { desktopSetup } from "./steps.ts";

const MCP = { key: "settings.mcp", command: { cmd: "mcp_servers" }, tags: ["mcp_servers"] } as const;
const MCP_REFRESH = { cmd: "mcp_servers", refresh: true };
import "./setup.css";

export function Welcome() {
  const app = useApp();
  const canSetUp = useMemo(() => desktopSetup(app) !== null, [app]);
  const caps = useCapabilities();
  const connections = Object.values(useSelector(selectConnections));
  const aether = connections.filter((c) => c.daemon === "aether");
  if (aether.some((c) => c.state === "ready")) {
    const local = canSetUp ? aether.find((c) => c.state === "ready" && app.manager.get(c.id)?.endpoint.transport.id === "ipc") : undefined;
    return <Connected archon={connections.some((c) => c.daemon === "archon" && c.state === "ready")} local={local?.id ?? null} />;
  }
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

/**
 * This computer's Aether may already run without Archon's tools, as when it was started before the
 * app's first launch: setup is not finished then, so the home offers to finish it first.
 */
function FinishSetup({ conn }: { readonly conn: ConnId }) {
  const app = useApp();
  const entry = useReread(conn, MCP.key, MCP.command, MCP.tags);
  const check = () => void app.reread(conn, MCP.key, MCP_REFRESH, MCP.tags);
  if (entry === undefined || (entry.value === null && entry.error === null)) return null;
  // An error is cleared only by a newer success, so one held here is newer than any cached value.
  if (entry.error !== null) return <CheckFailed conn={conn} text={`The app could not check Archon's tools: ${entry.error}`} onRetry={check} />;
  invariant(entry.value !== null, "a read with no error has a value");
  const servers = mcpServersOf(entry.value);
  if (servers === null) return <CheckFailed conn={conn} text="The app could not read the tool servers Aether reported." onRetry={check} />;
  const tools = archonTools(servers);
  const left = tools.state === "missing" || tools.state === "failed" ? tools : null;
  invariant(left === null || left.detail.trim() !== "", "an unfinished setup says what is left");
  const reason = left === null ? "" : `${left.detail} Connect them to finish setting up this computer.`;
  return <ConnectArchon needed={left !== null} reason={reason} origin={conn} onDone={check} />;
}

/** The check could not tell whether setup is finished: say so, offer to check again or run setup. */
function CheckFailed({ conn, text, onRetry }: { readonly conn: ConnId; readonly text: string; readonly onRetry: () => void }) {
  invariant(text.trim() !== "", "a failed check says why");
  return (
    <>
      <div className="su-repair" role="group" aria-label="Check Archon's tools">
        <div className="su-repair-head">
          <p className="su-repair-text" role="alert">{text}</p>
          <Button size="sm" onClick={onRetry}>Check again</Button>
          <a className="ui-link" href="#/setup">Set up on this computer</a>
        </div>
      </div>
      <ConnectArchon needed={false} reason="" origin={conn} onDone={onRetry} />
    </>
  );
}

function Connected({ archon, local }: { readonly archon: boolean; readonly local: ConnId | null }) {
  return (
    <div className="ui-page su-page">
      <section className="su-card su-welcome" aria-labelledby="su-home-title">
        <header className="su-hero">
          <p className="su-eyebrow">Ready</p>
          <h1 id="su-home-title" className="su-title">What should an agent do next?</h1>
          <p className="su-lede">Start a task and follow it as it runs.</p>
        </header>
        {local === null ? null : <FinishSetup conn={local} />}
        <footer className="su-actions">
          <a className="ui-button su-primary" data-variant="primary" data-size="md" href="#/activity/start">Start a task</a>
          <a className="ui-link" href="#/activity">See recent activity</a>
          {archon ? <a className="ui-link" href="#/archon/queue">Open Archon's queue</a> : null}
        </footer>
      </section>
    </div>
  );
}
