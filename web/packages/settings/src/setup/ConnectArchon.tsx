// "Connect Archon tools": the repair half of setup, offered wherever Archon's tools are missing
// from Aether. It runs the same connect, start and check steps as first-run setup.
// Callers keep it mounted and say whether it is needed: the repair restarts Aether, and its own
// rereads report the tools ready before it ends, so a started repair stays on screen to its outcome.

import { selectConnections, useApp, useReread, useSelector } from "@aether-ui/core";
import type { archon, DesktopSetup } from "@aether-ui/protocol";
import { Button } from "@aether-ui/ui";
import { useEffect, useMemo, useRef, type ReactNode } from "react";
import { invariant } from "../kit/invariant.ts";
import { archonHealth } from "./readiness.ts";
import { desktopSetup, type StepId } from "./steps.ts";
import { StepList } from "./StepList.tsx";
import { useSetup } from "./useSetup.ts";
import "./setup.css";

const REPAIR: readonly StepId[] = ["connect", "start", "check"];
const STATUS = { key: "setup.archon-status", command: { cmd: "status" }, tags: ["status"] } as const;

export interface ConnectArchonProps {
  /** Whether the tools need connecting now; a repair already started shows until it ends either way. */
  readonly needed: boolean;
  /** Why the tools need connecting, as the screen found it; empty when they do not. */
  readonly reason: string;
  /** Called once the tools are connected and checked, so the screen can read again. */
  readonly onDone?: (() => void) | undefined;
  /** Shows it as a banner above the screen rather than inline. */
  readonly banner?: boolean;
}

function framed(banner: boolean, content: ReactNode): ReactNode {
  return banner ? <aside className="su-banner" aria-label="Archon's tools">{content}</aside> : content;
}

export function ConnectArchon({ needed, reason, onDone, banner = false }: ConnectArchonProps) {
  invariant(!needed || reason.trim() !== "", "a needed repair says what it repairs");
  const app = useApp();
  const setup = useMemo(() => desktopSetup(app), [app]);
  if (setup === null) {
    if (!needed) return null;
    return framed(banner, <p className="su-repair-text">{reason} Connect them from the Aether desktop app on the computer that runs Aether.</p>);
  }
  return <ConnectArchonRun setup={setup} needed={needed} reason={reason} onDone={onDone} banner={banner} />;
}

function ConnectArchonRun({ setup, needed, reason, onDone, banner = false }: ConnectArchonProps & { readonly setup: DesktopSetup }) {
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
  if (!needed && !started) return null;
  invariant(started || reason.trim() !== "", "an offered repair says why");
  const headline = run.finished ? "Archon's tools are connected and ready." : reason;
  return framed(banner, (
    <div className="su-repair" role="group" aria-label="Connect Archon tools">
      {started && headline === "" ? null : (
        <div className="su-repair-head">
          <p className="su-repair-text">{headline}</p>
          {started ? null : <Button variant="primary" size="sm" onClick={() => void run.run()}>Connect Archon tools</Button>}
        </div>
      )}
      {started ? <StepList run={run} /> : null}
    </div>
  ));
}

/**
 * Above Archon's screens: says so, with the fix, when Archon reports its tools are not in Aether.
 * Shows nothing while Archon is not connected or its tools are ready or still starting, unless a
 * repair started here is still running or has an outcome to show.
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
  const view = status !== null && typeof status === "object" ? archonHealth(status.mcp) : null;
  const needed = view !== null && (view.state === "missing" || view.state === "failed");
  const reason = view !== null && view.state !== "ready" ? `${view.detail} Archon's runs cannot use its tools until they are connected.` : "";
  const reread = () => void app.reread(conn, STATUS.key, STATUS.command, STATUS.tags);
  return <ConnectArchon needed={needed} reason={reason} onDone={reread} banner />;
}
