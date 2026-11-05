// The "learn from this" sheet: it previews the evidence gathered about one entity, each fact with its
// status, and starts a teaching session from it. While Noesis or engineering context is off it reads nothing.

import { navigate, routeHref } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { Button, EmptyState, ErrorState, Sheet, Skeleton, StatusPill, type Status } from "@aether-ui/ui";
import { useCallback, useMemo, useState, useSyncExternalStore, type FormEvent } from "react";
import { useGate, useNoesisChange, useOperateRefusal, type Gate } from "../learn/data.ts";
import { classifyRefusal, invariant } from "../learn/model.ts";
import { RefusalNote } from "../learn/RefusalNote.tsx";
import { BUNDLE_LIMIT_BYTES, checkBundle, formatBytes, parseUnderstand, type BundleCheck, type UnderstandRequest } from "./bundle.ts";
import { SOURCES } from "./sources.ts";
import "./understand.css";

const GOAL_MAX = 2000;

export const STATUS_PILL: Readonly<Record<aether.EvidenceStatus, { readonly status: Status; readonly label: string; readonly hint: string }>> = {
  verified: { status: "succeeded", label: "Verified", hint: "A check or a merge established it" },
  reviewed: { status: "pr-merged", label: "Reviewed", hint: "A review or an operator gate passed it" },
  preliminary: { status: "queued", label: "Preliminary", hint: "A brief, an agent's words or work still in flight" },
};

function subscribeHash(listener: () => void): () => void {
  addEventListener("hashchange", listener);
  return () => removeEventListener("hashchange", listener);
}
const readHash = (): string => location.hash;

/** Mounted by `#/learn`: opens the sheet while the hash carries `?understand=…`; closing clears the query. */
export function UnderstandFromHash() {
  const hash = useSyncExternalStore(subscribeHash, readHash, readHash);
  const request = useMemo(() => parseUnderstand(hash), [hash]);
  const close = useCallback(() => navigate(hash.split("?", 1)[0] || "#/learn"), [hash]);
  if (request === null) return null;
  return <UnderstandSheet key={`${request.kind}:${request.conn}:${request.id}`} request={request} onClose={close} />;
}

export function UnderstandSheet({ request, onClose }: { readonly request: UnderstandRequest; readonly onClose: () => void }) {
  const gate = useGate();
  // The selected daemon's own setting: the capability is true when any connected daemon has it on.
  const open = gate.state === "on" && gate.status.effective.engineering_context === true;
  return (
    <Sheet open onClose={onClose} title="Learn from this" className="understand-sheet"
      description="Noesis teaches from the evidence below, read with your own access. Nothing is sent until you start.">
      {open ? <Preview conn={gate.conn} request={request} /> : <OffNote gate={gate} />}
    </Sheet>
  );
}

function OffNote({ gate }: { readonly gate: Gate }) {
  switch (gate.state) {
    case "loading":
      return <div className="ui-loading understand-loading" aria-busy="true" aria-label="Loading Noesis"><Skeleton height="var(--space-8)" /><Skeleton height="var(--space-12)" /></div>;
    case "disconnected":
      return <EmptyState title="Not connected to Aether">Learning runs on your Aether daemon. This opens once the app reconnects.</EmptyState>;
    case "unsupported":
      return <EmptyState title="This daemon has no Noesis">Update the Aether daemon to learn from your work.</EmptyState>;
    case "off":
      return <div data-testid="understand-off"><EmptyState title="Noesis is off">Learning from your work needs Noesis, which the daemon's host has not turned on. Nothing is read or sent.</EmptyState></div>;
    case "on":
      return (
        <div data-testid="understand-off">
          <EmptyState title="Learning from your work is off">
            The daemon's host has not turned on engineering context for Noesis. Until then nothing about this work is read or sent.
          </EmptyState>
        </div>
      );
  }
}

function Preview({ conn, request }: { readonly conn: string; readonly request: UnderstandRequest }) {
  const useSource = SOURCES[request.kind];
  invariant(useSource !== undefined, `a bundle source serves ${request.kind}`);
  const read = useSource(request.conn, request.id);
  switch (read.state) {
    case "loading":
      return <div className="ui-loading understand-loading" aria-busy="true" aria-label="Gathering the evidence"><Skeleton height="var(--space-6)" width="60%" /><Skeleton height="var(--space-12)" /><Skeleton height="var(--space-12)" /></div>;
    case "missing":
      return <EmptyState title={read.title}>{read.message}</EmptyState>;
    case "error":
      return read.retry ? <ErrorState title={read.title} message={read.message} onRetry={read.retry} /> : <ErrorState title={read.title} message={read.message} />;
    case "ready":
      return <BundlePreview key={read.bundle.title} conn={conn} bundle={read.bundle} />;
  }
}

function SizeMeter({ check }: { readonly check: BundleCheck }) {
  const share = Math.min(1, check.bytes / BUNDLE_LIMIT_BYTES);
  return (
    <div className="understand-size" data-over={check.ok ? undefined : "true"}>
      <div className="understand-meter" role="meter" aria-label="Evidence size" aria-valuemin={0} aria-valuemax={BUNDLE_LIMIT_BYTES} aria-valuenow={check.bytes}>
        <span className="understand-meter-fill" style={{ inlineSize: `${(share * 100).toFixed(1)}%` }} />
      </div>
      <span className="understand-size-text">{formatBytes(check.bytes)} of {formatBytes(BUNDLE_LIMIT_BYTES)}</span>
    </div>
  );
}

function EvidenceList({ bundle }: { readonly bundle: aether.EvidenceBundle }) {
  return (
    <ol className="understand-items" aria-label="Evidence">
      {bundle.items.map((item, index) => {
        const pill = STATUS_PILL[item.status];
        return (
          <li key={`${item.ref.kind}:${item.ref.id}:${index}`} className="understand-item" data-status={item.status}>
            <div className="understand-item-head">
              <span className="understand-ref"><span className="understand-ref-kind">{item.ref.kind}</span> <code>{item.ref.id}</code></span>
              <span title={pill.hint}><StatusPill status={pill.status} label={pill.label} /></span>
            </div>
            <p className="understand-excerpt">{item.excerpt}</p>
          </li>
        );
      })}
    </ol>
  );
}

function BundlePreview({ conn, bundle }: { readonly conn: string; readonly bundle: aether.EvidenceBundle }) {
  const check = useMemo(() => checkBundle(bundle), [bundle]);
  const [goal, setGoal] = useState(`Understand ${bundle.title}`);
  const start = useStart(conn, bundle, check);
  const refusal = useOperateRefusal(conn);
  const trimmed = goal.trim();
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (trimmed === "" || trimmed.length > GOAL_MAX) return;
    void start.send(trimmed);
  };
  return (
    <form className="understand" onSubmit={submit} aria-label="Start a session from this evidence">
      <div className="understand-goal">
        <label className="understand-label" htmlFor="understand-goal">What do you want to understand?</label>
        <textarea id="understand-goal" className="learn-textarea" rows={2} maxLength={GOAL_MAX} value={goal} onChange={(e) => setGoal(e.target.value)} />
      </div>
      <div className="understand-summary">
        <h3 className="understand-title">{bundle.title}</h3>
        <SizeMeter check={check} />
      </div>
      {!check.ok && <p className="learn-callout" role="alert" data-testid="understand-refused">{check.reason} It will not be sent.</p>}
      <EvidenceList bundle={bundle} />
      {start.failure}
      <div className="understand-actions">
        {refusal !== null && <span className="learn-muted">{refusal}</span>}
        <Button variant="primary" type="submit" disabled={start.busy || !check.ok || trimmed === "" || refusal !== null} title={refusal ?? undefined}>
          {start.busy ? "Starting…" : "Start learning"}
        </Button>
      </div>
    </form>
  );
}

type UnderstandRefusal = { readonly kind: "engineering" | "noesis" } | { readonly kind: "model" | "other"; readonly message: string };

/** The daemon answers `unavailable` with a reason when Noesis or engineering context was turned off meanwhile. */
export function classifyUnderstandRefusal(message: string): UnderstandRefusal {
  if (/engineering_context_disabled|engineering context/i.test(message)) return { kind: "engineering" };
  if (/noesis_disabled|noesis is disabled/i.test(message)) return { kind: "noesis" };
  return classifyRefusal(message);
}

function RefusalView({ refusal }: { readonly refusal: UnderstandRefusal }) {
  if (refusal.kind === "model" || refusal.kind === "other") return <RefusalNote refusal={refusal} />;
  const what = refusal.kind === "engineering" ? "Learning from your work was turned off" : "Noesis was turned off";
  return (
    <div className="learn-callout" role="status" data-kind={refusal.kind}>
      <p className="learn-callout-title">{what}</p>
      <p>The daemon's host changed the setting after this sheet opened. Nothing was started.</p>
    </div>
  );
}

function useStart(conn: string, bundle: aether.EvidenceBundle, check: BundleCheck) {
  const change = useNoesisChange(conn);
  const [busy, setBusy] = useState(false);
  const [refusal, setRefusal] = useState<UnderstandRefusal | null>(null);
  const send = async (goal: string) => {
    invariant(check.ok, "a refused bundle is never sent");
    invariant(goal.length > 0, "a session names its goal");
    setBusy(true);
    setRefusal(null);
    const result = await change({ cmd: "noesis_create_session", goal, mode: "teach", context: bundle });
    setBusy(false);
    if (!result.ok) return setRefusal(classifyUnderstandRefusal(result.error));
    const data = result.reply.data as { id?: unknown } | undefined;
    invariant(typeof data?.id === "string", "noesis_create_session answers with the new session");
    navigate(routeHref("#/learn/sessions/:id", { id: data.id }));
  };
  return { busy, send, failure: refusal === null ? null : <RefusalView refusal={refusal} /> };
}
