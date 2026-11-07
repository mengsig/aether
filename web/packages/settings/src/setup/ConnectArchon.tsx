// "Connect Archon tools": the repair half of setup, offered wherever Archon's tools are missing
// from Aether. It runs the same connect, start and check steps as first-run setup.
// The repair restarts Aether, which can change the connection or the screen a view hangs on, so the
// run is the app's (runs.ts): every view that offers it shows the same run until it ends or fails.

import { selectConnections, useApp, useReread, useSelector } from "@aether-ui/core";
import type { archon, ConnId } from "@aether-ui/protocol";
import { Button } from "@aether-ui/ui";
import { useEffect, type ReactNode } from "react";
import { invariant } from "../kit/invariant.ts";
import { archonHealth } from "./readiness.ts";
import { REPAIR_STEPS, useSetupRuns } from "./runs.ts";
import { StepList } from "./StepList.tsx";
import { useSetupRunner, type SetupRun, type SetupRunner } from "./useSetup.ts";
import "./setup.css";

const STATUS = { key: "setup.archon-status", command: { cmd: "status" }, tags: ["status"] } as const;
const ELSEWHERE = "Connect them from the desktop app on the computer running Aether.";

export interface ConnectArchonProps {
  /** Whether the tools need connecting now; a repair that is running or failed shows either way. */
  readonly needed: boolean;
  /** Why the tools need connecting, as the screen found it; empty when they do not. */
  readonly reason: string;
  /** The tools belong to another computer's Aether, so this app only says where to connect them. */
  readonly remote?: boolean;
  /** The connection the screen shows, which Settings keeps showing while the repair runs. */
  readonly origin?: ConnId | undefined;
  /** Called when a repair finishes while this view shows it, so the screen can read again. */
  readonly onDone?: (() => void) | undefined;
  /** Shows it as a banner above the screen rather than inline. */
  readonly banner?: boolean;
}

function framed(banner: boolean, content: ReactNode): ReactNode {
  return banner ? <aside className="su-banner" aria-label="Archon's tools">{content}</aside> : content;
}

export function ConnectArchon(props: ConnectArchonProps) {
  invariant(!props.needed || props.reason.trim() !== "", "a needed repair says what it repairs");
  const repair = useSetupRuns()?.repair;
  if (repair === undefined) return props.needed ? framed(props.banner ?? false, <p className="su-repair-text">{props.reason} {ELSEWHERE}</p>) : null;
  return <ConnectArchonRun runner={repair} {...props} />;
}

function ConnectArchonRun({ runner, needed, reason, remote = false, origin, onDone, banner = false }: ConnectArchonProps & { readonly runner: SetupRunner }) {
  const run = useSetupRunner(runner);
  invariant(run.steps === REPAIR_STEPS, "the repair view follows the repair run");
  useEffect(() => (onDone === undefined ? undefined : runner.whenFinished(onDone)), [runner, onDone]);
  const failed = REPAIR_STEPS.some((step) => run.states[step].state === "failed");
  if (run.busy || failed) return framed(banner, <RepairPanel headline={remote ? "" : reason} run={run} />);
  if (!needed) return null;
  if (remote) return framed(banner, <p className="su-repair-text">{reason} {ELSEWHERE}</p>);
  return framed(banner, <RepairPanel headline={reason} run={run} onStart={() => void run.run(REPAIR_STEPS[0], origin)} />);
}

/** The offer, or the run under way with each step's progress and any failure with its fix. */
function RepairPanel({ headline, run, onStart }: { readonly headline: string; readonly run: SetupRun; readonly onStart?: () => void }) {
  invariant(onStart !== undefined || run.busy || run.steps.some((step) => run.states[step].state === "failed"), "only an idle repair is offered");
  invariant(onStart === undefined || headline.trim() !== "", "an offered repair says why");
  return (
    <div className="su-repair" role="group" aria-label="Connect Archon tools">
      {headline === "" ? null : (
        <div className="su-repair-head">
          <p className="su-repair-text">{headline}</p>
          {onStart === undefined ? null : <Button variant="primary" size="sm" onClick={onStart}>Connect Archon tools</Button>}
        </div>
      )}
      {onStart === undefined ? <StepList run={run} /> : null}
    </div>
  );
}

/**
 * Above Archon's screens: says so, with the fix, when Archon reports its tools are not in Aether.
 * Shows nothing while Archon's tools are ready or still starting, unless a repair is under way or failed.
 */
export function ArchonToolsNotice() {
  const connections = useSelector(selectConnections);
  const archonConn = Object.values(connections).find((c) => c.daemon === "archon" && c.state === "ready");
  if (archonConn === undefined) return <ConnectArchon needed={false} reason="" banner />;
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
