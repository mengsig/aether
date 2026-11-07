// "Connect Archon tools": the repair half of setup, offered wherever Archon's tools are missing
// from Aether. It runs the same connect, start and check steps as first-run setup.

import { selectConnections, useApp, useReread, useSelector } from "@aether-ui/core";
import type { archon, DesktopSetup } from "@aether-ui/protocol";
import { Button } from "@aether-ui/ui";
import { useEffect, useMemo, useRef } from "react";
import { invariant } from "../kit/invariant.ts";
import { archonHealth } from "./readiness.ts";
import { desktopSetup, type StepId } from "./steps.ts";
import { StepList } from "./StepList.tsx";
import { useSetup } from "./useSetup.ts";
import "./setup.css";

const REPAIR: readonly StepId[] = ["connect", "start", "check"];
const STATUS = { key: "setup.archon-status", command: { cmd: "status" }, tags: ["status"] } as const;

export interface ConnectArchonProps {
  /** Why the tools need connecting, as the screen found it. */
  readonly reason: string;
  /** Called once the tools are connected and checked, so the screen can read again. */
  readonly onDone?: (() => void) | undefined;
}

export function ConnectArchon({ reason, onDone }: ConnectArchonProps) {
  invariant(reason.trim() !== "", "a repair says what it repairs");
  const app = useApp();
  const setup = useMemo(() => desktopSetup(app), [app]);
  if (setup === null) {
    return <p className="su-repair-text">{reason} Connect them from the Aether desktop app on the computer that runs Aether.</p>;
  }
  return <ConnectArchonRun setup={setup} reason={reason} onDone={onDone} />;
}

function ConnectArchonRun({ setup, reason, onDone }: ConnectArchonProps & { readonly setup: DesktopSetup }) {
  const app = useApp();
  const run = useSetup(setup, REPAIR);
  const reported = useRef(false);
  useEffect(() => {
    if (!run.finished || reported.current) return;
    reported.current = true;
    app.store.report("Archon's tools are connected and ready", undefined, undefined, "success");
    onDone?.();
  }, [app, run.finished, onDone]);
  const started = REPAIR.some((step) => run.states[step].state !== "waiting");
  return (
    <div className="su-repair" role="group" aria-label="Connect Archon tools">
      <div className="su-repair-head">
        <p className="su-repair-text">{run.finished ? "Archon's tools are connected and ready." : reason}</p>
        {started ? null : <Button variant="primary" size="sm" onClick={() => void run.run()}>Connect Archon tools</Button>}
      </div>
      {started ? <StepList run={run} /> : null}
    </div>
  );
}

/**
 * Above Archon's screens: says so, with the fix, when Archon reports its tools are not in Aether.
 * Shows nothing while Archon is not connected or its tools are ready or still starting.
 */
export function ArchonToolsNotice() {
  const connections = useSelector(selectConnections);
  const archonConn = Object.values(connections).find((c) => c.daemon === "archon" && c.state === "ready");
  if (archonConn === undefined) return null;
  return <ArchonHealthNotice conn={archonConn.id} />;
}

function ArchonHealthNotice({ conn }: { readonly conn: string }) {
  const app = useApp();
  const entry = useReread(conn, STATUS.key, STATUS.command, STATUS.tags);
  const status = entry?.value as archon.DaemonStatus | null | undefined;
  if (status === null || status === undefined || typeof status !== "object") return null;
  const view = archonHealth(status.mcp);
  if (view.state === "ready" || view.state === "starting") return null;
  const reread = () => void app.reread(conn, STATUS.key, STATUS.command, STATUS.tags);
  return (
    <aside className="su-banner" aria-label="Archon's tools">
      <ConnectArchon reason={`${view.detail} Archon's runs cannot use its tools until they are connected.`} onDone={reread} />
    </aside>
  );
}
