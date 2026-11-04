// Runs a read that writes into core's store and keeps only how it went: the screens select what it
// stored. A failure stays visible beside whatever good value core still holds.

import { DaemonError } from "@aether-ui/core";
import { useCallback, useEffect, useState } from "react";
import { invariant } from "../queue/model.ts";

export interface ReadFailure { readonly code: string; readonly message: string }

export interface StoreRead {
  readonly failure: ReadFailure | null;
  readonly reload: () => void;
}

export function failureOf(error: unknown): ReadFailure {
  if (error instanceof DaemonError) return { code: error.code, message: error.message };
  return { code: "failed", message: error instanceof Error ? error.message : String(error) };
}

/**
 * Runs `read` when it changes, when `signal` moves (by identity) and on reload; null reads nothing.
 * `read` is memoized by the caller on what it reads, so a new target is a new read.
 */
export function useStoreRead(read: (() => Promise<void>) | null, signal: unknown): StoreRead {
  invariant(read === null || typeof read === "function", "a store read is a function or nothing");
  const [outcome, setOutcome] = useState<{ read: unknown; failure: ReadFailure | null }>({ read: null, failure: null });
  const [nonce, setNonce] = useState(0);
  useEffect(() => {
    if (read === null) return;
    let current = true;
    read().then(
      () => { if (current) setOutcome({ read, failure: null }); },
      (error: unknown) => { if (current) setOutcome({ read, failure: failureOf(error) }); },
    );
    return () => { current = false; };
  }, [read, signal, nonce]);
  const reload = useCallback(() => setNonce((n) => n + 1), []);
  // A failure belongs to the read that failed; another target starts clean.
  return { failure: outcome.read === read ? outcome.failure : null, reload };
}
