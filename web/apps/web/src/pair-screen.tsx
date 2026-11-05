import { normalizeDeviceName, useApp } from "@aether-ui/core";
import type { RouteProps } from "@aether-ui/protocol";
import { Button, EmptyState, ErrorState, Field } from "@aether-ui/ui";
import { useEffect, useMemo, useState, useSyncExternalStore, type FormEvent } from "react";
import { forgetPairCode, offeredPairCode } from "./pair-link.ts";

type Daemon = "aether" | "archon";
const NAMES: Readonly<Record<Daemon, string>> = { aether: "Aether", archon: "Archon" };

function isDaemon(value: string | undefined): value is Daemon {
  return value === "aether" || value === "archon";
}

/** `#/pair/:daemon`: enter the host's code, then confirm the phrase the host shows. */
export default function PairScreen({ params }: RouteProps) {
  const daemon = params.daemon;
  if (!isDaemon(daemon)) return <EmptyState title="Unknown daemon">There is no daemon called “{daemon}” to pair with.</EmptyState>;
  return <Pairing key={daemon} daemon={daemon} />;
}

function Pairing({ daemon }: { readonly daemon: Daemon }) {
  const app = useApp();
  const flow = useMemo(() => app.pairing(daemon), [app, daemon]);
  const state = useSyncExternalStore(flow.subscribe, flow.state, flow.state);
  const [code, setCode] = useState(() => offeredPairCode(daemon));
  const [name, setName] = useState("This browser");
  useEffect(() => () => flow.cancel(), [flow]);

  const [nameError, setNameError] = useState<string | null>(null);
  const submit = (event: FormEvent) => {
    event.preventDefault();
    let device: string;
    try {
      device = normalizeDeviceName(name);
    } catch (error) {
      setNameError(error instanceof Error ? error.message : String(error));
      return;
    }
    setNameError(null);
    forgetPairCode();
    // PairingFlow turns every failure into its "failed" state, which this screen renders.
    void flow.start(code.trim(), device);
  };
  if (state.phase === "paired") {
    return <EmptyState title={`Paired with ${NAMES[daemon]}`} action={<a className="ui-link" href="#/">Open the overview</a>}>This device can now reach the daemon.</EmptyState>;
  }
  if (state.phase === "failed") {
    const message = state.message.trim() === "" ? "The daemon refused without giving a reason." : state.message;
    return <ErrorState title="Pairing failed" message={message} retryLabel="Start again" onRetry={() => flow.cancel()} />;
  }
  if (state.phase === "confirm") {
    return (
      <section className="app-pair" aria-live="polite">
        <p className="ui-eyebrow app-eyebrow">Confirm on the host</p>
        <p className="app-pair-phrase">{state.phrase}</p>
        <p className="app-pair-fingerprint">Fingerprint <code>{state.fingerprint}</code></p>
        <Button variant="quiet" onClick={() => flow.cancel()}>Cancel</Button>
      </section>
    );
  }
  const busy = state.phase === "claiming";
  return (
    <form className="app-pair" onSubmit={submit} aria-busy={busy}>
      <p className="ui-eyebrow app-eyebrow">Pair with {NAMES[daemon]}</p>
      <p className="app-pair-lede">Run <code>{daemon} remote pair</code> on the host and type the code it shows.</p>
      <Field label="Pairing code" placeholder="ABCD-1234" value={code} onChange={(event) => setCode(event.currentTarget.value)} required autoComplete="one-time-code" spellCheck={false} />
      <Field label="Name for this device" value={name} onChange={(event) => setName(event.currentTarget.value)} required
        {...(nameError === null ? {} : { error: nameError })} />
      <Button type="submit" variant="primary" disabled={busy || code.trim() === ""}>{busy ? "Pairing…" : "Pair"}</Button>
    </form>
  );
}
