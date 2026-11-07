// What became of a new chat's undelivered first message, said the same way in the start sheet and
// in the chat itself, with a retry that sends the saved text as typed to that same chat.

import type { ConnId } from "@aether-ui/protocol";
import { aether } from "@aether-ui/protocol";
import { Button } from "@aether-ui/ui";
import { useState } from "react";
import { Action, RunNote } from "../controls/kit.tsx";
import { invariant } from "../fleet/invariant.ts";
import { firstMessageKey, forgetUndelivered, readUndelivered, useFirstSend, type Undelivered } from "./firstMessage.ts";

/** The outcome, its reason, the recovery guidance and the saved text, quoted verbatim. */
export function UndeliveredNote({ record, keepProblem = null }: { readonly record: Undelivered; readonly keepProblem?: string | null }) {
  invariant(record.text !== "", "an undelivered first message has its text");
  return (
    <div className="b3-stack" role="alert" data-state={record.outcome}>
      {record.outcome === "refused"
        ? <p className="b3-note" data-state="failed">The first message was not delivered: {record.reason}</p>
        : <p className="b3-note" data-state="unconfirmed">
          The first message's reply never came, so it may have arrived. Check the chat before sending it again;
          the lost send waits in the <a className="ui-link" href="#/inbox">recovery review</a>.
        </p>}
      {keepProblem === null ? null : <p className="b3-note" data-state="failed">{keepProblem}</p>}
      <blockquote className="b3-quote" data-verbatim aria-label="First message, as typed">{record.text}</blockquote>
    </div>
  );
}

/** The kept record for one chat; it follows `key` when the screen switches agent. */
function useUndelivered(key: string): readonly [Undelivered | null, (record: Undelivered | null) => void] {
  const [held, setHeld] = useState(() => ({ key, record: readUndelivered(key) }));
  const current = held.key === key ? held : { key, record: readUndelivered(key) };
  if (current !== held) setHeld(current);
  return [current.record, (record) => setHeld({ key, record })];
}

/** In a chat: its first message was refused or unconfirmed at start; send it again or let it go. */
export function UndeliveredFirst({ conn, agent }: { readonly conn: ConnId; readonly agent: string }) {
  const key = firstMessageKey(conn, agent);
  const [record, setRecord] = useUndelivered(key);
  const [keepProblem, setKeepProblem] = useState<string | null>(null);
  const first = useFirstSend(conn);
  if (record === null) return null;
  const resend = async () => {
    const result = await first.send(agent, record.text);
    invariant(result.outcome === "sent" || result.text === record.text, "a resend reports on the text it sent");
    setRecord(result.outcome === "sent" ? null : { text: result.text, outcome: result.outcome, reason: result.reason });
    setKeepProblem(result.outcome === "sent" ? null : result.keepProblem);
  };
  const dismiss = () => {
    forgetUndelivered(key);
    setRecord(null);
  };
  return (
    <section className="conv-first" aria-label="First message not delivered">
      <UndeliveredNote record={record} keepProblem={keepProblem} />
      <div className="b3-row">
        <Action conn={conn} command={aether.sendMessage({ to: agent, content: record.text })} label="Send it as typed" variant="primary"
          busy={first.run.status.state === "busy"} onRun={resend} />
        <Button size="sm" variant="quiet" onClick={dismiss}>Dismiss</Button>
      </div>
      {first.run.status.state === "busy" ? <RunNote status={first.run.status} /> : null}
    </section>
  );
}
