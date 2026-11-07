// "Connect Archon tools": the repair half of setup, offered wherever Archon's tools are missing
// from Aether. It runs the same connect, start and check steps as first-run setup.
// The repair restarts Aether, which can change the connection or the screen a view hangs on, so the
// run belongs to the app: every view that offers it shows the same run until it ends or fails.

import { selectConnections, useApp, useReread, useSelector, type App } from "@aether-ui/core";
import type { archon, DesktopSetup } from "@aether-ui/protocol";
import { Button } from "@aether-ui/ui";
import { useEffect, useMemo, useRef, type ReactNode } from "react";
import { invariant } from "../kit/invariant.ts";
import { archonHealth } from "./readiness.ts";
import { desktopSetup, type StepId } from "./steps.ts";
import { StepList } from "./StepList.tsx";
import { SetupRunner, useSetupRunner, type SetupRun } from "./useSetup.ts";
import "./setup.css";

const REPAIR: readonly StepId[] = ["connect", "start", "check"];
const STATUS = { key: "setup.archon-status", command: { cmd: "status" }, tags: ["status"] } as const;
const CONNECTED = "Archon's tools are connected and ready";
const ELSEWHERE = "Connect them from the desktop app on the computer running Aether.";

/** Each app's one repair run; the success notice comes from the run, so no view has to outlive it. */
const repairs = new WeakMap<App, SetupRunner>();

function repairOf(app: App, setup: DesktopSetup): SetupRunner {
  const known = repairs.get(app);
  if (known !== undefined) return known;
  const runner = new SetupRunner(app, setup, REPAIR, () => app.store.report(CONNECTED, undefined, undefined, "success"));
  repairs.set(app, runner);
  return runner;
}

export interface ConnectArchonProps {
  /** Whether the tools need connecting now; a repair that is running or failed shows either way. */
  readonly needed: boolean;
  /** Why the tools need connecting, as the screen found it; empty when they do not. */
  readonly reason: string;
  /** The tools belong to another computer's Aether, so this app only says where to connect them. */
  readonly remote?: boolean;
  /** Called once the tools are connected and checked, so the screen can read again. */
  readonly onDone?: (() => void) | undefined;
  /** Shows it as a banner above the screen rather than inline. */
  readonly banner?: boolean;
}

function framed(banner: boolean, content: ReactNode): ReactNode {
  return banner ? <aside className="su-banner" aria-label="Archon's tools">{content}</aside> : content;
}

export function ConnectArchon({ needed, reason, remote = false, onDone, banner = false }: ConnectArchonProps) {
  invariant(!needed || reason.trim() !== "", "a needed repair says what it repairs");
  const app = useApp();
  const setup = useMemo(() => desktopSetup(app), [app]);
  if (setup === null) return needed ? framed(banner, <p className="su-repair-text">{reason} {ELSEWHERE}</p>) : null;
  const runner = repairOf(app, setup);
  return <ConnectArchonRun runner={runner} needed={needed} reason={reason} remote={remote} onDone={onDone} banner={banner} />;
}

function ConnectArchonRun({ runner, needed, reason, remote = false, onDone, banner = false }: ConnectArchonProps & { readonly runner: SetupRunner }) {
  const run = useSetupRunner(runner);
  const doneSeen = useRef(false);
  useEffect(() => {
    if (!run.finished) doneSeen.current = false;
    if (!run.finished || doneSeen.current) return;
    doneSeen.current = true;
    onDone?.();
  }, [run.finished, onDone]);
  const failed = REPAIR.some((step) => run.states[step].state === "failed");
  if (run.busy || failed) return framed(banner, <RepairPanel headline={remote ? "" : reason} run={run} />);
  if (!needed) return null;
  if (remote) return framed(banner, <p className="su-repair-text">{reason} {ELSEWHERE}</p>);
  return framed(banner, <RepairPanel headline={reason} run={run} onStart={() => void run.run(REPAIR[0])} />);
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
