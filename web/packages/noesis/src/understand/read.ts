// What a bundle source answers, and the one way a source asks core to (re)read: on mount, when its
// target changes and on retry, only while the entity's connection is ready.

import { selectConnection, useApp, useSelector, type App } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { useCallback, useEffect, useMemo, useState } from "react";
import { errorText } from "../learn/data.ts";
import { invariant } from "../learn/model.ts";

export type BundleRead =
  | { readonly state: "loading" }
  /** Nothing to build from: not connected, not found, or not read yet. */
  | { readonly state: "missing"; readonly title: string; readonly message: string }
  | { readonly state: "error"; readonly title: string; readonly message: string; readonly retry?: () => void }
  | { readonly state: "ready"; readonly bundle: aether.EvidenceBundle };

/** A hook that builds one kind's bundle from core's store: `(entity conn, entity id) => BundleRead`. */
export type BundleSource = (conn: string, id: string) => BundleRead;

export type Reader = (app: App, conn: string, targets: readonly string[]) => Promise<void>;

export interface CoreRead {
  /** Why the newest read failed, until a later one answers. */
  readonly error: string | null;
  /** Whether a read of these targets has answered, with data or an error. */
  readonly settled: boolean;
  readonly retry: () => void;
}

export function useConnectionReady(conn: string, daemon: "aether" | "archon"): boolean {
  invariant(conn.length > 0, "a source reads one connection");
  const info = useSelector(useMemo(() => selectConnection(conn), [conn]));
  return info?.state === "ready" && info.daemon === daemon;
}

/** Runs `reader` for `targets` (null reads nothing); the failure is kept to show, never dropped. */
export function useCoreRead(conn: string, ready: boolean, targets: readonly string[] | null, reader: Reader): CoreRead {
  const app = useApp();
  const key = targets === null ? null : JSON.stringify(targets);
  const [answer, setAnswer] = useState<{ key: string; error: string | null } | null>(null);
  const [nonce, setNonce] = useState(0);
  useEffect(() => {
    if (!ready || key === null) return;
    const parsed = JSON.parse(key) as string[];
    invariant(parsed.every((target) => target.length > 0), "every read target is named");
    let live = true;
    reader(app, conn, parsed).then(
      () => { if (live) setAnswer({ key, error: null }); },
      (error: unknown) => { if (live) setAnswer({ key, error: errorText(error) }); },
    );
    return () => { live = false; };
  }, [app, conn, ready, key, reader, nonce]);
  const retry = useCallback(() => setNonce((n) => n + 1), []);
  const mine = answer !== null && answer.key === key;
  return { error: mine ? answer.error : null, settled: mine, retry };
}

export const notConnected = (daemon: "Aether" | "Archon", conn: string): BundleRead =>
  ({ state: "missing", title: `Not connected to ${daemon}`, message: `${conn} is not connected. The evidence is read once it reconnects.` });
