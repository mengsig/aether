// The desktop main process for web/e2e's flows: the real startDesktop, offscreen, on the stack's
// sockets. Playwright drives its windows; main's own state for the specs sits on globalThis.e2e.

import { existsSync } from "node:fs";
import path from "node:path";
import { app, session } from "electron";
import { Bridge } from "../../src/bridge/bridge.ts";
import { registerAppScheme, startDesktop, type Desktop } from "../../src/main/start.ts";
import type { Confirmation } from "../../src/security/confirm.ts";
import { guardSession } from "../../src/security/navigation.ts";
import { remotePartition } from "../../src/security/origins.ts";

/** One call the app window made on the bridge, as main received it. */
interface BridgeCall { readonly method: string; readonly args: readonly unknown[] }

/** A refusal the next request for `cmd` gets instead of reaching the daemon, as the daemon words one. */
interface Fault { readonly cmd: string; readonly code: string; readonly message: string }

/** What a spec reads and scripts in main through ElectronApplication.evaluate. */
interface E2eMain {
  desktop?: Desktop;
  readonly calls: BridgeCall[];
  readonly confirmations: Confirmation[];
  /** Answers the next native confirmations take, in order; none left refuses. */
  readonly answers: boolean[];
  /** Refusals queued by a spec, each taken by the next request for its command. */
  readonly faults: Fault[];
}

function required(name: string): string {
  const value = process.env[name] ?? "";
  if (!path.isAbsolute(value)) throw new Error(`${name} names an absolute path, not ${JSON.stringify(value)}`);
  return value;
}

/**
 * Test only: the remote origin's own partition trusts the front's one certificate, by fingerprint.
 * No other session and no global switch is touched.
 */
function trustFront(): void {
  const origin = process.env.E2E_DESKTOP_TRUST_ORIGIN;
  const fingerprint = process.env.E2E_DESKTOP_TRUST_FINGERPRINT;
  if (origin === undefined || origin === "") return;
  if (fingerprint === undefined || !fingerprint.startsWith("sha256/")) throw new Error("a trusted origin comes with its certificate's sha256 fingerprint");
  const partition = session.fromPartition(remotePartition(origin));
  // Made here before any remote window, so it gets that window's guards at once.
  guardSession(partition);
  partition.setCertificateVerifyProc((request, callback) => {
    callback(request.certificate.fingerprint === fingerprint ? 0 : -3);
  });
}

/** The daemon's refusal frame for a request the spec scripted to fail, or null to send it on. */
function faulted(faults: Fault[], method: string, args: readonly unknown[]): Record<string, unknown> | null {
  if (method !== "request") return null;
  const frame = args[1] as { cmd?: unknown; req?: unknown } | undefined;
  const index = faults.findIndex((fault) => fault.cmd === frame?.cmd);
  if (index < 0) return null;
  const [fault] = faults.splice(index, 1);
  if (fault === undefined || typeof frame?.req !== "number") throw new Error("a scripted refusal answers a numbered request");
  return { frame: "reply", id: frame.req, result: { Err: { code: fault.code, message: fault.message } } };
}

/** Every IPC call, from the window's first: the bridge's one entry is wrapped before it exists. */
function recordCalls(state: E2eMain): void {
  const handle = Bridge.prototype.handle;
  Bridge.prototype.handle = function record(this: Bridge, sender, owner, method, args) {
    state.calls.push({ method, args: structuredClone(args) });
    const refusal = faulted(state.faults, method, args);
    return refusal === null ? handle.call(this, sender, owner, method, args) : Promise.resolve(refusal);
  };
}

const dir = required("E2E_DESKTOP_DIR");
if (!existsSync(dir)) throw new Error(`E2E_DESKTOP_DIR ${dir} exists`);
const state: E2eMain = { calls: [], confirmations: [], answers: [], faults: [] };
(globalThis as { e2e?: E2eMain }).e2e = state;
app.setPath("userData", path.join(dir, "user-data"));
app.setPath("logs", path.join(dir, "logs"));
app.setPath("crashDumps", path.join(dir, "crashes"));
registerAppScheme();
app.whenReady()
  .then(async () => {
    trustFront();
    recordCalls(state);
    const env = { HOME: dir, PATH: "/usr/bin:/bin", AETHER_SOCKET: required("E2E_DESKTOP_AETHER_SOCKET"), ARCHON_SOCKET: required("E2E_DESKTOP_ARCHON_SOCKET") };
    const confirm = async (_owner: number, shown: Confirmation): Promise<boolean> => {
      state.confirmations.push(shown);
      return state.answers.shift() ?? false;
    };
    const desktop = await startDesktop({ offscreen: true, env, confirm });
    state.desktop = desktop;
  })
  .catch((error: unknown) => {
    process.stderr.write(`the e2e desktop failed to start: ${error instanceof Error ? (error.stack ?? error.message) : String(error)}\n`);
    app.exit(2);
  });
