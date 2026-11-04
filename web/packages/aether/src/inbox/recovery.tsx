// The recovery review: every change whose reply never came. Nothing is retried on its own; the
// operator asks the daemon what became of it (`resolve_submission`), then sends it again or discards it.

import { selectAgents, selectConnections, selectOutbox, selectRead, selectReview, useApp, useSelector, type State, type Submission } from "@aether-ui/core";
import type { ConnId } from "@aether-ui/protocol";
import { Button, Dialog } from "@aether-ui/ui";
import { useEffect, useMemo, useState } from "react";
import { invariant } from "../fleet/invariant.ts";
import { Action, RunNote, Section, useRun } from "../controls/kit.tsx";
import { describeCommand } from "./describe.ts";

/** How many submissions wait for the operator: unconfirmed outbox entries and the review. */
export function unresolvedCount(state: State): number {
  let count = state.review.length;
  for (const conn of [...Object.values(state.aether), ...Object.values(state.archon)]) count += conn.outbox.filter((s) => s.state === "unknown").length;
  return count;
}

function Discard({ conn, submission, title }: { readonly conn: ConnId; readonly submission: Submission; readonly title: string }) {
  const app = useApp();
  const [asking, setAsking] = useState(false);
  return (
    <>
      <Button variant="quiet" size="sm" onClick={() => setAsking(true)}>Discard</Button>
      <Dialog open={asking} onClose={() => setAsking(false)} title={`Discard ${title}?`}
        description="It leaves this review. If it did happen on the daemon, it stays done; nothing is undone or sent."
        footer={<><Button variant="quiet" onClick={() => setAsking(false)}>Keep it</Button><Button variant="danger" data-autofocus onClick={() => app.settle(conn, submission.key)}>Discard</Button></>} />
    </>
  );
}

type Resolution = { readonly outcome: "executed" | "absent_safe" | "unknown"; readonly reason?: unknown };

const OUTCOME_TEXT: Readonly<Record<Resolution["outcome"], string>> = {
  executed: "It happened. The daemon has its reply, so it is cleared from the review.",
  absent_safe: "It never reached the daemon. Sending it again is safe.",
  unknown: "The daemon cannot tell whether it happened. Sending it again is safe only if doing it twice is harmless.",
};

/** One unconfirmed submission on a live connection: check, then send again or discard. */
function Unconfirmed({ conn, submission, names }: { readonly conn: ConnId; readonly submission: Submission; readonly names: Readonly<Record<string, string>> }) {
  invariant(submission.state === "unknown" && submission.key !== "", "the review shows unconfirmed, keyed submissions");
  const app = useApp();
  const run = useRun(conn);
  const key = `submission:${submission.key}`;
  const check = useMemo(() => ({ cmd: "resolve_submission", command: submission.command, ...(submission.horizon ? { horizon: submission.horizon } : {}) }),
    [submission.command, submission.horizon]);
  const read = useSelector(useMemo(() => selectRead(conn, key), [conn, key]));
  const resolution = (read?.value ?? null) as Resolution | null;
  const { title, detail } = describeCommand(submission.command, names);
  useEffect(() => {
    if (resolution?.outcome === "executed") app.settle(conn, submission.key);
  }, [app, conn, resolution?.outcome, submission.key]);
  const again = resolution !== null && resolution.outcome !== "executed";
  return (
    <li className="b3-item">
      <div className="b3-item-head">
        <p className="b3-item-title">{title}</p>
        <span className="b3-meta">{submission.error ?? "No reply"}</span>
      </div>
      {detail ? <p className="b3-prose">{detail}</p> : null}
      {resolution ? <p className="b3-note" role="status" data-state={resolution.outcome === "absent_safe" ? "done" : "unconfirmed"}>{OUTCOME_TEXT[resolution.outcome]}</p> : null}
      {read?.error ? <p className="b3-note" role="alert" data-state="failed">The check failed: {read.error}</p> : null}
      <div className="b3-row">
        <Action conn={conn} command={check} label={resolution ? "Check again" : "Check what happened"} busy={read !== undefined && read.requested > read.issued && read.error === null}
          onRun={() => app.reread(conn, key, check, ["submission_resolved"])} />
        {again ? (
          <Action conn={conn} command={submission.command} label="Send again" variant={resolution.outcome === "unknown" ? "danger" : "primary"} busy={run.status.state === "busy"}
            confirm={{ title: `Send ${title} again?`, body: `${OUTCOME_TEXT[resolution.outcome]} It goes with the same key, so a daemon that did run it does not run it twice.`, confirmLabel: "Send again" }}
            onRun={() => run.send(submission.command, "Sent again and confirmed")} />
        ) : null}
        <Discard conn={conn} submission={submission} title={title} />
      </div>
      <RunNote status={run.status} />
    </li>
  );
}

function ConnectionOutbox({ conn }: { readonly conn: ConnId }) {
  const outbox = useSelector(useMemo(() => selectOutbox(conn), [conn]));
  const agents = useSelector(useMemo(() => selectAgents(conn), [conn]));
  const names = useMemo(() => Object.fromEntries(Object.values(agents).map((a) => [a.id, a.name])), [agents]);
  const lost = outbox.filter((s) => s.state === "unknown");
  return <>{lost.map((s) => <Unconfirmed key={s.key} conn={conn} submission={s} names={names} />)}</>;
}

/** A submission from a daemon identity that is gone: it cannot be checked, only discarded. */
function Orphan({ submission }: { readonly submission: Submission }) {
  const { title, detail } = describeCommand(submission.command, {});
  return (
    <li className="b3-item">
      <div className="b3-item-head"><p className="b3-item-title">{title}</p><span className="b3-meta">{submission.conn}</span></div>
      {detail ? <p className="b3-prose">{detail}</p> : null}
      <p className="b3-note" data-state="unconfirmed">The daemon was replaced before it answered, so whether this happened cannot be checked from here. Look at the agent before doing it again.</p>
      <div className="b3-row"><Discard conn={submission.conn} submission={submission} title={title} /></div>
    </li>
  );
}

export function RecoveryReview() {
  const count = useSelector(unresolvedCount);
  const connections = useSelector(selectConnections);
  const review = useSelector(selectReview);
  invariant(count >= review.length, "the count includes every orphaned submission");
  if (count === 0) return null;
  return (
    <Section title="Recovery review" hint={`${count} ${count === 1 ? "change" : "changes"} sent without a reply. Nothing here is sent again unless you choose to.`}>
      <ul className="b3-list" aria-label="Recovery review">
        {Object.keys(connections).map((conn) => <ConnectionOutbox key={conn} conn={conn} />)}
        {review.map((s) => <Orphan key={`${s.conn}:${s.key}`} submission={s} />)}
      </ul>
    </Section>
  );
}
