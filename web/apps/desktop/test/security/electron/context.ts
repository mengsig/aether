// What every headless check shares: the started desktop, the fixtures, the scripted confirmation
// answers, and helpers that run a check and evaluate page script with a deadline.

import type { WebContents, WebPreferences } from "electron";
import type { Desktop } from "../../../src/main/start.ts";
import type { Confirmation } from "../../../src/security/confirm.ts";
import type { FakeDaemon, RemoteServer } from "./fixtures.ts";

export interface CheckResult {
  readonly name: string;
  readonly ok: boolean;
  readonly ms: number;
  readonly error?: string;
}

/**
 * The preferences the page actually runs with, from Electron's internal accessor (untyped); the
 * page-side checks (no require, no process, no bridge) back it up.
 */
export function effectivePreferences(contents: WebContents): WebPreferences {
  const read = (contents as unknown as { getLastWebPreferences?: () => WebPreferences | null }).getLastWebPreferences;
  if (typeof read !== "function") throw new Error("this Electron has no getLastWebPreferences");
  const preferences = read.call(contents);
  if (preferences === null) throw new Error("the page has no preferences yet");
  return preferences;
}

const SCRIPT_TIMEOUT_MS = 10_000;
const SETTLE_MS = 400;

export class Context {
  readonly results: CheckResult[] = [];
  /** Answers the next confirmations take, in order; a confirmation with none left is a failure. */
  readonly answers: boolean[] = [];
  readonly confirmations: Confirmation[] = [];
  desktop!: Desktop;
  readonly daemon: FakeDaemon;
  readonly server: RemoteServer;

  constructor(daemon: FakeDaemon, server: RemoteServer) {
    this.daemon = daemon;
    this.server = server;
  }

  /** The harness's confirm: records what main showed and gives the scripted answer. */
  confirm = async (_owner: number, shown: Confirmation): Promise<boolean> => {
    this.confirmations.push(shown);
    const answer = this.answers.shift();
    if (answer === undefined) throw new Error("a confirmation was shown that no check expected");
    return answer;
  };

  async check(name: string, body: () => Promise<void>): Promise<void> {
    if (this.results.some((result) => result.name === name)) throw new Error(`two checks are named ${name}`);
    const start = Date.now();
    try {
      await body();
      this.results.push({ name, ok: true, ms: Date.now() - start });
    } catch (error) {
      this.results.push({ name, ok: false, ms: Date.now() - start, error: error instanceof Error ? (error.stack ?? error.message) : String(error) });
    }
  }

  /** Evaluates `script` in the page and fails after a deadline instead of hanging the run. */
  async js(contents: WebContents, script: string): Promise<unknown> {
    if (contents.isDestroyed()) throw new Error("the page is gone");
    let timer: NodeJS.Timeout | undefined;
    const deadline = new Promise<never>((_resolve, reject) => {
      timer = setTimeout(() => reject(new Error(`page script timed out: ${script.slice(0, 80)}`)), SCRIPT_TIMEOUT_MS);
    });
    try {
      return await Promise.race([contents.executeJavaScript(script, true), deadline]);
    } finally {
      clearTimeout(timer);
    }
  }

  settle(ms = SETTLE_MS): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, ms));
  }

  /** Polls `test` until it holds or the deadline passes. */
  async until(what: string, test: () => boolean, ms = SCRIPT_TIMEOUT_MS): Promise<void> {
    const end = Date.now() + ms;
    while (!test()) {
      if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
      await this.settle(50);
    }
  }
}
