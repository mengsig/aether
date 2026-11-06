// Drafts: what a person typed into a form, composer or filter, kept across screen changes until the
// form submits or clears it. Keyed by `<screen>/<form>`, held in memory and mirrored to
// sessionStorage so a reload of the same tab keeps them too; nothing here reaches a daemon.

import { useCallback, useSyncExternalStore } from "react";

export type DraftValue = Readonly<Record<string, unknown>>;
type Listener = () => void;

/** Where drafts are mirrored; the browser's sessionStorage, or nothing in tests and workers. */
export interface DraftMirror { getItem(key: string): string | null; setItem(key: string, value: string): void; removeItem(key: string): void }

export const DRAFT_STORAGE_KEY = "aether.drafts.v1";
/** A draft bigger than this is not mirrored: sessionStorage is small and such a draft is likely pasted data. */
const MIRROR_LIMIT_BYTES = 256 * 1024;

function invariant(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(`@aether-ui/core drafts: ${message}`);
}

function sessionMirror(): DraftMirror | null {
  try {
    const storage = (globalThis as { sessionStorage?: DraftMirror }).sessionStorage;
    return storage ?? null;
  } catch {
    // Reading `sessionStorage` throws under some privacy settings; the drafts then live in memory only.
    return null;
  }
}

function parseMirror(text: string | null): Map<string, DraftValue> {
  if (text === null || text === "") return new Map();
  try {
    const parsed: unknown = JSON.parse(text);
    if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) return new Map();
    return new Map(Object.entries(parsed as Record<string, DraftValue>).filter(([, value]) => value !== null && typeof value === "object"));
  } catch {
    // A corrupt mirror is dropped rather than blocking every form; the next write overwrites it.
    return new Map();
  }
}

export class DraftStore {
  readonly #drafts: Map<string, DraftValue>;
  readonly #listeners = new Map<string, Set<Listener>>();
  readonly #mirror: DraftMirror | null;

  constructor(mirror: DraftMirror | null = sessionMirror()) {
    this.#mirror = mirror;
    this.#drafts = parseMirror(mirror?.getItem(DRAFT_STORAGE_KEY) ?? null);
  }

  /** The draft under `key`, or undefined when nothing was typed yet. */
  get(key: string): DraftValue | undefined {
    checkKey(key);
    return this.#drafts.get(key);
  }

  /** Replaces the draft under `key`; an empty draft (every field at its initial value) is cleared instead. */
  set(key: string, value: DraftValue): void {
    checkKey(key);
    invariant(value !== null && typeof value === "object" && !Array.isArray(value), `draft ${key} is a record`);
    this.#drafts.set(key, value);
    this.#notify(key);
  }

  clear(key: string): void {
    checkKey(key);
    if (!this.#drafts.delete(key)) return;
    this.#notify(key);
  }

  /** Every key with a draft; for tests and the devtools, not for screens. */
  keys(): string[] {
    return [...this.#drafts.keys()];
  }

  subscribe(key: string, listener: Listener): () => void {
    checkKey(key);
    const listeners = this.#listeners.get(key) ?? new Set();
    listeners.add(listener);
    this.#listeners.set(key, listeners);
    return () => {
      listeners.delete(listener);
      if (listeners.size === 0) this.#listeners.delete(key);
    };
  }

  #notify(key: string): void {
    this.#persist();
    for (const listener of this.#listeners.get(key) ?? []) listener();
  }

  #persist(): void {
    if (this.#mirror === null) return;
    const text = JSON.stringify(Object.fromEntries(this.#drafts));
    if (text.length > MIRROR_LIMIT_BYTES) return;
    try {
      if (this.#drafts.size === 0) this.#mirror.removeItem(DRAFT_STORAGE_KEY);
      else this.#mirror.setItem(DRAFT_STORAGE_KEY, text);
    } catch {
      // A full or disabled sessionStorage loses only the reload mirror; the in-memory draft stands.
    }
  }
}

function checkKey(key: string): void {
  invariant(/^[a-z0-9-]+\/[a-z0-9:_./-]+$/i.test(key), `draft key ${JSON.stringify(key)} is <screen>/<form>`);
}

/** The app's one draft store; `useDraft` reads it, tests make their own. */
export const drafts = new DraftStore();

/** Whether every field of `value` equals its initial value, so the draft can be dropped. */
export function isPristine<T extends DraftValue>(value: T, initial: T): boolean {
  const keys = new Set([...Object.keys(value), ...Object.keys(initial)]);
  for (const key of keys) {
    if (!Object.is(value[key], initial[key])) return false;
  }
  return true;
}

export type DraftPatch<T> = Partial<T> | ((current: T) => T);

export interface Draft<T extends DraftValue> {
  readonly value: T;
  /** Merges a patch, or replaces through a function of the current draft. */
  readonly update: (patch: DraftPatch<T>) => void;
  /** Forgets the draft: after a submit, or when the person discards it. */
  readonly clear: () => void;
  /** Whether anything differs from `initial`. */
  readonly dirty: boolean;
}

/**
 * A form's draft under `key` (`<screen>/<form>`), seeded from `initial` and kept until cleared. Fields
 * absent from a stored draft take their initial value, so a form may grow new fields safely.
 */
export function useDraft<T extends DraftValue>(key: string, initial: T, store: DraftStore = drafts): Draft<T> {
  const stored = useSyncExternalStore(
    useCallback((listener: Listener) => store.subscribe(key, listener), [store, key]),
    useCallback(() => store.get(key), [store, key]),
  );
  const value = stored === undefined ? initial : { ...initial, ...(stored as Partial<T>) };
  const update = useCallback((patch: DraftPatch<T>) => {
    const current = { ...initial, ...((store.get(key) ?? {}) as Partial<T>) };
    const next = typeof patch === "function" ? patch(current) : { ...current, ...patch };
    if (isPristine(next, initial)) store.clear(key);
    else store.set(key, next);
  }, [store, key, initial]);
  const clear = useCallback(() => store.clear(key), [store, key]);
  return { value, update, clear, dirty: stored !== undefined };
}
