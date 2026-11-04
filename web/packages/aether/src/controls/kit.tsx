// The pieces every B3 control shares: a command button that knows whether this device may send it
// (disabled with the reason shown, host-only marked), an explicit confirm step for destructive or
// privileged actions, and the outcome of a run. Nothing here holds daemon data.

import { codeOf, usePermission, useApp, useCommand, type LocalAction } from "@aether-ui/core";
import type { ConnId } from "@aether-ui/protocol";
import { Badge, Button, Dialog, type ButtonVariant } from "@aether-ui/ui";
import { useCallback, useId, useState, type ReactNode } from "react";
import { invariant } from "../fleet/invariant.ts";
import "./kit.css";

type Json = Record<string, unknown>;

/** A reply that never came: the command may or may not have acted, so it waits in the recovery review. */
const UNCONFIRMED = new Set(["timeout", "disconnected"]);

export type RunStatus =
  | { readonly state: "idle" | "busy"; readonly message?: undefined }
  | { readonly state: "done" | "failed" | "unconfirmed"; readonly message: string };

export interface Run {
  readonly status: RunStatus;
  /** Sends one command through `useCommand`; resolves to its reply, or null when it failed. */
  readonly send: (command: Json, done?: string) => Promise<Json | null>;
  /** Runs a host-only action through the desktop's native confirmation. */
  readonly privileged: (action: LocalAction, done?: string) => Promise<boolean>;
}

export function useRun(conn: ConnId): Run {
  const app = useApp();
  const command = useCommand(conn);
  const [status, setStatus] = useState<RunStatus>({ state: "idle" });
  const fail = useCallback((error: unknown) => {
    const message = error instanceof Error ? error.message : String(error);
    if (UNCONFIRMED.has(codeOf(error) ?? "")) {
      setStatus({ state: "unconfirmed", message: "The reply never arrived, so it is not known whether this happened. It waits in the recovery review." });
    } else {
      setStatus({ state: "failed", message });
    }
  }, []);
  const send = useCallback(async (input: Json, done = "Done") => {
    invariant(typeof input.cmd === "string", "a control sends a named command");
    setStatus({ state: "busy" });
    try {
      const reply = await command(input);
      setStatus({ state: "done", message: done });
      return reply;
    } catch (error) {
      fail(error);
      return null;
    }
  }, [command, fail]);
  const privileged = useCallback(async (action: LocalAction, done = "Done") => {
    setStatus({ state: "busy" });
    try {
      await app.privileged(conn, action);
      setStatus({ state: "done", message: done });
      return true;
    } catch (error) {
      fail(error);
      return false;
    }
  }, [app, conn, fail]);
  return { status, send, privileged };
}

/** What became of the last run: announced politely, with the way to the recovery review when unconfirmed. */
export function RunNote({ status }: { readonly status: RunStatus }) {
  if (status.state === "idle") return null;
  if (status.state === "busy") return <p className="b3-note" role="status" data-state="busy">Sending…</p>;
  invariant(status.message !== "", "a finished run says how it went");
  return (
    <p className="b3-note" role={status.state === "done" ? "status" : "alert"} data-state={status.state}>
      {status.message}
      {status.state === "unconfirmed" ? <> <a className="ui-link" href="#/inbox">Open the recovery review</a></> : null}
    </p>
  );
}

export interface Confirm {
  readonly title: string;
  readonly body: string;
  readonly confirmLabel: string;
  /** Asks for a reason before confirming; `required` keeps the confirm button off until one is typed. */
  readonly reason?: "optional" | "required";
}

export interface ActionProps {
  readonly conn: ConnId;
  /** The command as it will be sent; it decides the access check, so pass it even for a privileged action. */
  readonly command: Json;
  readonly label: string;
  readonly onRun: (reason: string) => unknown;
  readonly confirm?: Confirm;
  readonly variant?: ButtonVariant;
  readonly busy?: boolean;
  /** Disables the control for a reason of the screen's own (nothing selected, nothing to change). */
  readonly blocked?: string | null;
}

/**
 * One command button. It is disabled, with the reason beside it, whenever core would refuse the
 * command for this device; a host-only command is marked; a `confirm` asks before anything is sent.
 */
export function Action({ conn, command, label, onRun, confirm, variant = "secondary", busy = false, blocked = null }: ActionProps) {
  const { access, refusal } = usePermission(conn, command);
  const [asking, setAsking] = useState(false);
  const [reason, setReason] = useState("");
  const reasonId = useId();
  const why = refusal ?? blocked;
  const start = () => {
    if (confirm) setAsking(true);
    else void onRun("");
  };
  const confirmed = () => {
    setAsking(false);
    void onRun(reason.trim());
    setReason("");
  };
  return (
    <span className="b3-action" data-refused={refusal === null ? undefined : "true"}>
      <Button variant={variant} size="sm" disabled={why !== null || busy} aria-describedby={why === null ? undefined : reasonId} onClick={start}>{label}</Button>
      {access === "host_only" ? <Badge tone="info">Host only</Badge> : null}
      {why === null ? null : <span className="b3-reason" id={reasonId}>{why}</span>}
      {confirm ? (
        <Dialog open={asking} onClose={() => setAsking(false)} title={confirm.title} description={confirm.body}
          footer={(
            <>
              <Button variant="quiet" onClick={() => setAsking(false)}>Keep it</Button>
              <Button variant={variant === "danger" ? "danger" : "primary"} data-autofocus disabled={confirm.reason === "required" && reason.trim() === ""} onClick={confirmed}>
                {confirm.confirmLabel}
              </Button>
            </>
          )}>
          {confirm.reason ? (
            <label className="b3-field">
              <span className="ui-field-label">{confirm.reason === "required" ? "Reason" : "Reason (optional)"}</span>
              <textarea className="b3-textarea" value={reason} rows={3} onChange={(event) => setReason(event.target.value)} />
            </label>
          ) : null}
        </Dialog>
      ) : null}
    </span>
  );
}

/** A titled group of controls on a calm surface. */
export function Section({ title, hint, children }: { readonly title: string; readonly hint?: string; readonly children: ReactNode }) {
  invariant(title.trim() !== "", "a section has a title");
  return (
    <section className="b3-section" aria-label={title}>
      <header className="b3-section-head">
        <h3 className="b3-section-title">{title}</h3>
        {hint === undefined ? null : <p className="b3-section-hint">{hint}</p>}
      </header>
      {children}
    </section>
  );
}

/** The standing banner: says once, at the top, why this device's controls are off. */
export function StandingNotice({ conn }: { readonly conn: ConnId }) {
  const { refusal } = usePermission(conn, { cmd: "send_message" });
  if (refusal === null) return null;
  return <p className="b3-standing" role="note">{refusal}. Every control here is read-only on this device.</p>;
}
