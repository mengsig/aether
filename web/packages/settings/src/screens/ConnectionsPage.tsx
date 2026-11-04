// Every daemon this app is connected to, from core's store. "Reconnect now" and "Forget this device"
// act on this app, not the daemon; "Pair again" goes to the pairing screen.

import { selectConnections, useApp, useSelector } from "@aether-ui/core";
import type { ConnectionInfo } from "@aether-ui/protocol";
import { Badge, Button, Dialog, EmptyState, type Tone } from "@aether-ui/ui";
import { useState } from "react";
import { invariant } from "../kit/invariant.ts";
import { Section } from "../kit/kit.tsx";
import { plural } from "./format.ts";

const STATE_TONE: Readonly<Record<ConnectionInfo["state"], Tone>> = {
  ready: "success", connecting: "info", pairing: "info", backoff: "warning", closed: "neutral",
};

const STANDING_TEXT = { local: "Local (this host)", operator: "Operator", observer: "Observer" } as const;

/** Removes this browser's saved device identity after asking; the daemon keeps the device until revoked. */
function ForgetDevice({ conn, label }: { readonly conn: string; readonly label: string }) {
  const app = useApp();
  const [asking, setAsking] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  invariant(conn !== "" && label !== "", "forgetting names its connection");
  const forget = async () => {
    setAsking(false);
    try {
      await app.forgetDevice(conn);
      setProblem(null);
      globalThis.location.hash = "#/pair/aether";
    } catch (error) {
      setProblem(`Could not forget this device: ${error instanceof Error ? error.message : String(error)}`);
    }
  };
  return (
    <>
      <Button size="sm" variant="danger" onClick={() => setAsking(true)}>Forget this device</Button>
      <Dialog open={asking} onClose={() => setAsking(false)} title={`Forget this device on ${label}?`}
        description="This removes the saved identity from this browser. You need a fresh pairing to reconnect; the daemon still lists the device until it is revoked on the host."
        footer={<><Button variant="quiet" onClick={() => setAsking(false)}>Keep it</Button><Button variant="danger" data-autofocus onClick={() => void forget()}>Forget this device</Button></>} />
      {problem === null ? null : <p className="st-note" role="alert" data-state="failed">{problem}</p>}
    </>
  );
}

function ConnectionRow({ info }: { readonly info: ConnectionInfo }) {
  const app = useApp();
  invariant(info.id !== "" && info.label !== "", "a connection has an id and a label");
  const standing = info.standing === undefined ? "Not known yet" : STANDING_TEXT[info.standing];
  const reconnect = () => {
    const connection = app.manager.get(info.id);
    invariant(connection !== undefined, `connection ${info.id} is known to the manager`);
    connection.retryNow();
  };
  return (
    <li className="st-item" aria-label={info.label}>
      <div className="st-item-head">
        <h4 className="st-item-title">{info.label}</h4>
        <Badge tone={STATE_TONE[info.state]}>{info.state}</Badge>
      </div>
      <dl className="st-facts">
        <div><dt>Daemon</dt><dd>{info.daemon === "aether" ? "Aether" : "Archon"}</dd></div>
        <div><dt>Protocol</dt><dd>{info.protocol === undefined ? "Unknown" : `v${info.protocol}`}</dd></div>
        <div><dt>Standing</dt><dd>{standing}</dd></div>
        <div><dt>Features</dt><dd>{plural(info.features.length, "feature")}</dd></div>
      </dl>
      <div className="st-row">
        <Button size="sm" variant="secondary" onClick={reconnect}>Reconnect now</Button>
        <a className="ui-link" href={`#/pair/${info.daemon}`}>Pair again</a>
        {app.manager.get(info.id)?.endpoint.transport.id === "ws-aether" ? <ForgetDevice conn={info.id} label={info.label} /> : null}
      </div>
    </li>
  );
}

export function ConnectionsPage() {
  const connections = Object.values(useSelector(selectConnections));
  if (connections.length === 0) {
    return <EmptyState title="No daemons yet" action={<a className="ui-link" href="#/pair/aether">Pair a daemon</a>}>This app is not connected to any daemon.</EmptyState>;
  }
  return (
    <Section title="Connections" hint="The daemons this app talks to, and what this device may do on each.">
      <ul className="st-list">
        {connections.map((info) => <ConnectionRow key={info.id} info={info} />)}
      </ul>
    </Section>
  );
}
