// Push notifications for this device. Whether push is on is the daemon's answer, read into core's
// store; the browser's own subscription is made and dropped on demand and never held here.

import { selectConnections, useApp, useReread, useSelector } from "@aether-ui/core";
import type { aether, ConnId } from "@aether-ui/protocol";
import { Badge, EmptyState } from "@aether-ui/ui";
import { invariant } from "../kit/invariant.ts";
import { Action, RunNote, Section, StandingNotice, useRun, type Run } from "../kit/kit.tsx";
import { pushSupported, subscribeBrowser, unsubscribeBrowser } from "./push.ts";
import { ReadView, recordOf, Unexpected } from "./read.tsx";

const KEY = "settings.push";
const COMMAND = { cmd: "get_push_config" };

interface PushControlsProps {
  readonly conn: ConnId;
  readonly config: aether.PushConfig;
  readonly run: Run;
  readonly reread: () => Promise<void>;
  readonly blocked: string | null;
}

// The browser step and the daemon command run apart, so a lost reply to the command reaches the
// recovery review through `run.send` instead of passing as a plain failure.
function PushControls({ conn, config, run, reread, blocked }: PushControlsProps) {
  const key = config.vapid_public_key;
  const subscribe = async () => {
    invariant(typeof key === "string" && key !== "", "the daemon gave a VAPID key to subscribe with");
    let subscription: aether.PushSubscription | null = null;
    const prepared = await run.attempt(async () => { subscription = await subscribeBrowser(key); }, "This browser is ready for notifications");
    if (!prepared) return;
    invariant(subscription !== null, "a prepared browser gave a subscription");
    const reply = await run.send({ cmd: "subscribe_push", subscription }, "Notifications are on for this device");
    if (reply !== null) await reread();
  };
  const unsubscribe = async () => {
    const reply = await run.send({ cmd: "unsubscribe_push" }, "Notifications are off for this device");
    if (reply === null) return;
    await run.attempt(unsubscribeBrowser, "Notifications are off for this device");
    await reread();
  };
  const test = () => run.send({ cmd: "test_push" }, "Test notification sent");
  if (!config.subscribed) {
    return <Action conn={conn} command={{ cmd: "subscribe_push" }} label="Turn on notifications" variant="primary" onRun={subscribe} blocked={blocked} />;
  }
  return (
    <div className="st-row">
      <Action conn={conn} command={{ cmd: "test_push" }} label="Send a test" onRun={test} blocked={blocked} />
      <Action conn={conn} command={{ cmd: "unsubscribe_push" }} label="Turn off notifications" variant="danger" onRun={unsubscribe} blocked={blocked}
        confirm={{ title: "Turn off notifications?", body: "This device stops receiving alerts from the daemon.", confirmLabel: "Turn off" }} />
    </div>
  );
}

function PushBody({ conn, value, run, reread }: Omit<PushControlsProps, "config" | "blocked"> & { readonly value: unknown }) {
  const standing = useSelector(selectConnections)[conn]?.standing;
  const config = recordOf<aether.PushConfig>(value);
  if (config === null || typeof config.enabled !== "boolean") return <Unexpected label="the push settings" />;
  if (!config.enabled || !config.vapid_public_key) {
    return <EmptyState title="Push is not available">The daemon has push turned off, so there is nothing to subscribe to. Turn push on in the daemon's configuration on its host, then come back.</EmptyState>;
  }
  const blocked = standing === "local" ? "Push is for paired devices; the desktop app has its own notifications" : pushSupported() ? null : "This browser cannot receive push notifications";
  return (
    <>
      <div className="st-row">
        <Badge tone={config.subscribed ? "success" : "neutral"}>{config.subscribed ? "On for this device" : "Off for this device"}</Badge>
        {config.previews ? <Badge tone="info">Shows previews</Badge> : null}
      </div>
      {config.kinds && config.kinds.length > 0 ? <p className="st-meta">Alerts for: {config.kinds.join(", ").replaceAll("_", " ")}</p> : null}
      <PushControls conn={conn} config={config} run={run} reread={reread} blocked={blocked} />
    </>
  );
}

export function NotificationsPage({ conn }: { readonly conn: ConnId }) {
  const app = useApp();
  const entry = useReread(conn, KEY, COMMAND, ["push_config"]);
  const run = useRun(conn);
  const reread = () => app.reread(conn, KEY, COMMAND, ["push_config"]);
  return (
    <div className="st-stack">
      <StandingNotice conn={conn} />
      <Section title="Notifications" hint="Alerts when an agent needs you, even with this page closed.">
        <ReadView label="notification settings" entry={entry} onRetry={() => void reread()}>
          {(value) => <PushBody conn={conn} value={value} run={run} reread={reread} />}
        </ReadView>
        <RunNote status={run.status} />
      </Section>
    </div>
  );
}
