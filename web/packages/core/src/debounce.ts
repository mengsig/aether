// Collapses repeated work per key into one call after a quiet period:
// several refetches of one entity within the window become one (§5.3).

import { invariant } from "./guards.ts";

export class Debouncer {
  private readonly timers = new Map<string, ReturnType<typeof setTimeout>>();
  private readonly delayMs: number;

  constructor(delayMs: number) {
    invariant(delayMs >= 0, "a debounce delay is nonnegative");
    this.delayMs = delayMs;
  }

  /** Runs `run` once, `delayMs` after the first schedule of `key` in a window. */
  schedule(key: string, run: () => void): void {
    if (this.timers.has(key)) return;
    this.timers.set(key, setTimeout(() => {
      this.timers.delete(key);
      run();
    }, this.delayMs));
  }

  cancelAll(): void {
    for (const timer of this.timers.values()) clearTimeout(timer);
    this.timers.clear();
  }

  get pending(): number {
    return this.timers.size;
  }
}

/** Runs tasks of one key one after another, so an older reply never lands after a newer one. */
export class KeyedSerial {
  private readonly tails = new Map<string, Promise<void>>();

  /** Returns `task`'s own outcome; a failed task does not stop the ones queued after it. */
  run(key: string, task: () => Promise<void>): Promise<void> {
    const next = (this.tails.get(key) ?? Promise.resolve()).then(task);
    // The caller gets the failure; later tasks only wait for it to settle.
    const tail = next.then(() => undefined, () => undefined);
    this.tails.set(key, tail);
    invariant(this.tails.size <= 10_000, "keyed tasks settle and are forgotten");
    void tail.then(() => {
      if (this.tails.get(key) === tail) this.tails.delete(key);
    });
    return next;
  }
}
