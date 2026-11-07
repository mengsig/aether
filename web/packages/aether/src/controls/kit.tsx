// The pieces every B3 control shares: a command button that knows whether this device may send it
// (disabled with the reason shown, host-only marked), an explicit confirm step for destructive or
// privileged actions, and the outcome of a run. Nothing here holds daemon data.

import { connectionDraftKey, drafts, useDraft, checkAllowed, codeOf, isKeyed, usePermission, useApp, useCommand, useSelector, type CommandInput, type LocalAction, type ReadEntry } from "@aether-ui/core";
import type { ConnId } from "@aether-ui/protocol";
import { Badge, Button, Dialog, TextArea, EmptyState, ErrorState, Skeleton, type ButtonVariant } from "@aether-ui/ui";
import { useCallback, useId, useState, type ReactNode } from "react";
import { invariant } from "../fleet/invariant.ts";
import "./kit.css";

type Json = Record<string, unknown>;

/** A reply that never came: the command may or may not have acted, so it waits in the recovery review. */
const UNCONFIRMED = new Set(["timeout", "disconnected"]);

export type RunStatus =
  | { readonly state: "idle" | "busy"; readonly message?: undefined }
  | { readonly state: "done" | "failed" | "unconfirmed"; readonly message: string };

/** One command's outcome: its reply, or why there is none (refused, or the reply never came). */
export type Attempt =
  | { readonly reply: Json; readonly status: { readonly state: "done"; readonly message: string } }
  | { readonly reply: null; readonly status: { readonly state: "failed" | "unconfirmed"; readonly message: string } };

export interface Run {
  readonly status: RunStatus;
  /** Sends one command through `useCommand`; resolves to its reply, or null when it failed. */
  readonly send: (command: CommandInput, done?: string) => Promise<Json | null>;
  /** As `send`, for a caller whose next step depends on whether the command was refused or lost. */
  readonly attempt: (command: CommandInput, done?: string) => Promise<Attempt>;
  /** Runs a host-only action through the desktop's native confirmation. */
  readonly privileged: (action: LocalAction, done?: string) => Promise<boolean>;
}

/** A refusal, or a lost reply: only a command core holds (one with a body) waits in the recovery review. */
function failure(error: unknown, message: string, held: boolean): Extract<Attempt, { reply: null }>["status"] {
  if (!UNCONFIRMED.has(codeOf(error) ?? "")) return { state: "failed", message };
  if (!held) return { state: "failed", message: "The reply never arrived, so it is not known whether this happened. Check its current state before trying again." };
  return { state: "unconfirmed", message: "The reply never arrived, so it is not known whether this happened. It waits in the recovery review." };
}

export function useRun(conn: ConnId): Run {
  const app = useApp();
  const command = useCommand(conn);
  const [status, setStatus] = useState<RunStatus>({ state: "idle" });
  /** A lost reply waits in the recovery review; core keeps keyed commands, an unkeyed one is noted there. */
  const fail = useCallback((error: unknown, command: Json | null): Extract<Attempt, { reply: null }>["status"] => {
    const message = error instanceof Error ? error.message : String(error);
    const status = failure(error, message, command !== null);
    if (status.state === "unconfirmed" && command !== null && !isKeyed(command)) app.noteUnconfirmed(conn, command, message || "No reply");
    setStatus(status);
    return status;
  }, [app, conn]);
  const attempt = useCallback(async (input: CommandInput, done = "Done"): Promise<Attempt> => {
    const json = input as Json;
    invariant(typeof json.cmd === "string", "a control sends a named command");
    setStatus({ state: "busy" });
    try {
      const reply = await command(json);
      invariant(reply !== null && typeof reply === "object", "a delivered command has a reply");
      drafts.clear(confirmationDraftKey(conn, json));
      const status = { state: "done" as const, message: done };
      setStatus(status);
      return { reply, status };
    } catch (error) {
      return { reply: null, status: fail(error, json) };
    }
  }, [command, fail, conn]);
  const send = useCallback(async (input: CommandInput, done?: string) => (await attempt(input, done)).reply, [attempt]);
  const privileged = useCallback(async (action: LocalAction, done = "Done") => {
    setStatus({ state: "busy" });
    try {
      await app.privileged(conn, action);
      setStatus({ state: "done", message: done });
      return true;
    } catch (error) {
      fail(error, null);
      return false;
    }
  }, [app, conn, fail]);
  return { status, send, attempt, privileged };
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
  readonly "data-testid"?: string;
  /** The command as it will be sent; it decides the access check, so pass it even for a privileged action. */
  readonly command: CommandInput;
  readonly label: string;
  readonly onRun: (reason: string) => unknown;
  readonly confirm?: Confirm | undefined;
  readonly variant?: ButtonVariant;
  readonly busy?: boolean;
  /** Disables the control for a reason of the screen's own (nothing selected, nothing to change). */
  readonly blocked?: string | null;
  /** Host-only for a reason the command itself does not carry (the stored record's scope). */
  readonly hostOnly?: boolean;
}

/** Core's own refusal for a host-only command on this connection, or null when this is the host. */
function useHostOnlyRefusal(conn: ConnId, hostOnly: boolean): string | null {
  const standing = useSelector(useCallback((state) => state.connections[conn]?.standing, [conn]));
  if (!hostOnly) return null;
  try {
    checkAllowed("host_only", standing);
    return null;
  } catch (error) {
    if (!(error instanceof Error)) throw error;
    return error.message;
  }
}

/**
 * One command button. It is disabled, with the reason beside it, whenever core would refuse the
 * command for this device; a host-only command is marked; a `confirm` asks before anything is sent.
 */
export function Action({ conn, command, label, onRun, confirm, variant = "secondary", busy = false, blocked = null, hostOnly = false, "data-testid": testId }: ActionProps) {
  const permission = usePermission(conn, command);
  const hostRefusal = useHostOnlyRefusal(conn, hostOnly);
  const access = hostOnly ? "host_only" : permission.access;
  const refusal = permission.refusal ?? hostRefusal;
  const [asking, setAsking] = useState(false);
  const reasonDraft = useDraft(confirmationDraftKey(conn, command as Json), { reason: "" });
  const { reason } = reasonDraft.value;
  const reasonId = useId();
  const why = refusal ?? blocked;
  const start = () => {
    if (confirm) setAsking(true);
    else void onRun("");
  };
  const confirmed = () => {
    setAsking(false);
    void onRun(reason.trim());
  };
  return (
    <span className="b3-action" data-refused={refusal === null ? undefined : "true"}>
      <Button data-testid={testId} variant={variant} size="sm" disabled={why !== null || busy} aria-describedby={why === null ? undefined : reasonId} onClick={start}>{label}</Button>
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
            <TextArea label={confirm.reason === "required" ? "Reason" : "Reason (optional)"} value={reason} rows={3}
              onChange={event => reasonDraft.update({ reason: event.target.value })} />
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
    <section className="ui-card b3-section" aria-label={title}>
      <header className="b3-section-head">
        <h3 className="ui-card-title b3-section-title">{title}</h3>
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

export interface ReadViewProps<T> {
  readonly entry: ReadEntry | undefined;
  /** What is being read, in words: "the schedules". */
  readonly what: string;
  readonly empty: string;
  /** Checks the stored reply has the shape this view renders; a mismatch is a protocol bug. */
  readonly guard: (value: unknown) => value is T;
  readonly isEmpty: (value: T) => boolean;
  readonly retry: () => void;
  readonly children: (value: T) => ReactNode;
}

/** Renders one store-held reread: loading, failed with a retry, empty, or its value (with a stale-read note). */
export function ReadView<T>({ entry, what, empty, guard, isEmpty, retry, children }: ReadViewProps<T>) {
  invariant(what.trim() !== "" && empty.trim() !== "", "a read view names what it reads");
  if (entry === undefined || entry.value === null) {
    if (entry?.error) return <ErrorState title={`Could not load ${what}`} message={entry.error} onRetry={retry} />;
    return <div className="b3-stack" aria-busy="true" aria-label={`Loading ${what}`}><Skeleton height="2.5rem" /><Skeleton height="2.5rem" /></div>;
  }
  invariant(guard(entry.value), `the stored reply for ${what} has the expected shape`);
  const value = entry.value;
  return (
    <>
      {entry.error ? <p className="b3-note" role="alert" data-state="failed">Showing the last answer; the newest read failed: {entry.error}</p> : null}
      {isEmpty(value) ? <EmptyState title={empty} /> : children(value)}
    </>
  );
}

export const isList = (value: unknown): value is readonly unknown[] => Array.isArray(value);

function confirmationDraftKey(conn: string, command: Json): string {
  invariant(conn.length > 0, "a confirmation belongs to a connection");
  invariant(typeof command.cmd === "string", "a confirmation names its command");
  const target = command.id ?? command.agent ?? command.to ?? command.target ?? command.scope ?? "all";
  const identity = encodeURIComponent(JSON.stringify(target)).replaceAll("%", "_").replace(/[!'()*~]/g, "_");
  return connectionDraftKey("controls", conn, `${command.cmd}/${identity}/confirmation`);
}
