// The one validation point of every IPC call (ARCHITECTURE.md §c). In order: the size limits,
// the sender (the top-level app:// document of an app window), the schema of each argument,
// then the class allowlist. A handler only ever sees the ValidCall this returns.

import type { PreferenceKey, PrivilegedAction } from "@aether-ui/protocol";
import type { ValidateFunction } from "ajv/dist/2020.js";
import { passThroughRefusal, type DaemonKind } from "./classes.ts";
import { APP_HOST, APP_SCHEME } from "./csp.ts";
import { MAX_FRAME_BYTES, MAX_STRING_BYTES, utf8Bytes } from "./limits.ts";
import { CONN, schemaError, type Schemas } from "./schema.ts";

/** A call main refuses; the renderer sees the message and nothing ran. */
export class BridgeRefusal extends Error {
  override readonly name = "BridgeRefusal";
}

/** Who sent an IPC message, as Electron reports it. */
export interface Sender {
  /** Main registered this webContents as an app window (never a remote window). */
  readonly isAppWindow: boolean;
  /** The sending frame is its page's main frame. */
  readonly isTopFrame: boolean;
  readonly frameUrl: string | undefined;
}

export const PREFERENCE_KEYS = ["window.bounds", "layout.panels", "theme", "desktop.managed_start", "remote.origins"] as const;

export type Frame = Readonly<Record<string, unknown>>;

export type ValidCall =
  | { method: "discover" | "diagnostics" }
  | { method: "connect"; daemon: string }
  | { method: "request" | "subscribe"; conn: string; frame: Frame; text: string }
  | { method: "close"; conn: string }
  | { method: "privileged"; action: PrivilegedAction }
  | { method: "startDaemon"; kind: DaemonKind }
  | { method: "pickDirectory"; purpose: string }
  | { method: "notify"; note: { title: string; body: string; route?: string } }
  | { method: "openExternal"; url: string }
  | { method: "openRemote"; origin: string }
  | { method: "getPreference"; key: PreferenceKey }
  | { method: "setPreference"; key: PreferenceKey; value: unknown };

export type BridgeMethod = ValidCall["method"];

const TEXT = { type: "string", minLength: 1 };
const ARGUMENTS: Readonly<Record<BridgeMethod, readonly unknown[]>> = {
  discover: [],
  diagnostics: [],
  connect: [{ type: "string", pattern: "^(aether|archon):[a-z0-9-]{1,32}$" }],
  request: [CONN, { type: "object" }],
  subscribe: [CONN, { type: "object" }],
  close: [CONN],
  privileged: [{ type: "object" }],
  startDaemon: [{ enum: ["aether", "archon"] }],
  pickDirectory: [TEXT],
  notify: [{
    type: "object", additionalProperties: false, required: ["title", "body"],
    properties: { title: TEXT, body: { type: "string" }, route: { type: "string", pattern: "^/" } },
  }],
  openExternal: [TEXT],
  openRemote: [TEXT],
  getPreference: [{ enum: PREFERENCE_KEYS }],
  setPreference: [{ enum: PREFERENCE_KEYS }, {}],
};

export const BRIDGE_METHODS: readonly BridgeMethod[] = Object.keys(ARGUMENTS) as BridgeMethod[];

/** What validation needs from the bridge's state: which connections exist and to what. */
export interface Connections {
  kindOf(conn: string): DaemonKind | undefined;
}

export class Validator {
  private readonly schemas: Schemas;
  private readonly argumentChecks: ReadonlyMap<string, ValidateFunction>;

  constructor(schemas: Schemas) {
    this.schemas = schemas;
    this.argumentChecks = new Map(BRIDGE_METHODS.map((method) => {
      const items = ARGUMENTS[method];
      const tuple = { type: "array", prefixItems: items, items: false, minItems: items.length };
      return [method, schemas.ajv.compile(tuple)];
    }));
  }

  /** The checked call, or a BridgeRefusal naming the first rule it breaks. */
  check(sender: Sender, method: string, args: readonly unknown[], connections: Connections): ValidCall {
    const argumentCheck = this.argumentChecks.get(method);
    if (argumentCheck === undefined) throw new BridgeRefusal(`Unknown bridge method ${method.slice(0, 64)}`);
    if (!Array.isArray(args)) throw new BridgeRefusal("bridge arguments are a list");
    const text = checkSizes(method as BridgeMethod, args);
    checkSender(sender);
    if (!argumentCheck(args)) throw new BridgeRefusal(`${method}: ${schemaError(argumentCheck)}`);
    return this.typed(method as BridgeMethod, args, text, connections);
  }

  private typed(method: BridgeMethod, args: readonly unknown[], text: string | undefined, connections: Connections): ValidCall {
    const [first, second] = args;
    switch (method) {
      case "discover": case "diagnostics": return { method };
      case "connect": return { method, daemon: first as string };
      case "close": return { method, conn: first as string };
      case "request": case "subscribe": return this.frameCall(method, first as string, text, connections);
      case "privileged": return { method, action: this.privilegedAction(first, connections) };
      case "startDaemon": return { method, kind: first as DaemonKind };
      case "pickDirectory": return { method, purpose: first as string };
      case "notify": return { method, note: first as { title: string; body: string } };
      case "openExternal": return { method, url: first as string };
      case "openRemote": return { method, origin: first as string };
      case "getPreference": return { method, key: first as PreferenceKey };
      case "setPreference": return { method, key: first as PreferenceKey, value: second };
    }
  }

  private frameCall(method: "request" | "subscribe", conn: string, text: string | undefined, connections: Connections): ValidCall {
    if (text === undefined) throw new BridgeRefusal("a frame call carries its frame text");
    const kind = connections.kindOf(conn);
    if (kind === undefined) throw new BridgeRefusal(`${conn} is not an open local connection`);
    const frame = JSON.parse(text) as Frame;
    if (!Number.isSafeInteger(frame.req) || (frame.req as number) < 1) {
      throw new BridgeRefusal("A frame carries its request id in req, a positive integer");
    }
    const invalid = this.schemas.frames[kind].check(frame);
    if (invalid !== undefined) throw new BridgeRefusal(invalid);
    const refusal = passThroughRefusal(kind, frame);
    if (refusal !== undefined) throw new BridgeRefusal(refusal);
    return { method, conn, frame, text };
  }

  private privilegedAction(value: unknown, connections: Connections): PrivilegedAction {
    if (!this.schemas.privileged(value)) {
      const kind = (value as { kind?: unknown }).kind;
      throw new BridgeRefusal(`Not a privileged action (${String(kind).slice(0, 64)}): ${schemaError(this.schemas.privileged)}`);
    }
    const action = value as PrivilegedAction;
    const kind = connections.kindOf(action.conn);
    if (kind === undefined) throw new BridgeRefusal("Privileged actions run only on a local connection");
    const wanted = action.kind === "archon.operator_change" ? "archon" : "aether";
    if (kind !== wanted) throw new BridgeRefusal(`${action.kind} runs on an ${wanted} connection, not ${kind}`);
    return action;
  }
}

/** The frame's JSON text, or a refusal; also bounds every other string and the whole call. */
function checkSizes(method: BridgeMethod, args: readonly unknown[]): string | undefined {
  const text = jsonText(args);
  if (utf8Bytes(text) > MAX_FRAME_BYTES + MAX_STRING_BYTES) throw new BridgeRefusal("The call exceeds the 1 MiB frame limit");
  if (method === "request" || method === "subscribe") {
    const frame = jsonText(args[1]);
    if (utf8Bytes(frame) > MAX_FRAME_BYTES) throw new BridgeRefusal("The frame exceeds the 1 MiB limit");
    checkStrings(args[0], 0);
    return frame;
  }
  for (const arg of args) checkStrings(arg, 0);
  return undefined;
}

function jsonText(value: unknown): string {
  let text: string | undefined;
  try {
    text = JSON.stringify(value);
  } catch (error) {
    throw new BridgeRefusal("Bridge arguments are JSON", { cause: error });
  }
  if (text === undefined) throw new BridgeRefusal("Bridge arguments are JSON");
  return text;
}

const MAX_DEPTH = 16;

function checkStrings(value: unknown, depth: number): void {
  if (depth > MAX_DEPTH) throw new BridgeRefusal("The argument nests too deeply");
  if (typeof value === "string") {
    if (utf8Bytes(value) > MAX_STRING_BYTES) throw new BridgeRefusal("A string argument exceeds 4 KiB");
    return;
  }
  if (typeof value !== "object" || value === null) return;
  for (const [key, item] of Object.entries(value)) {
    checkStrings(key, depth + 1);
    checkStrings(item, depth + 1);
  }
}

function checkSender(sender: Sender): void {
  if (!sender.isAppWindow) throw new BridgeRefusal("Only the app window may use the desktop bridge");
  if (!sender.isTopFrame) throw new BridgeRefusal("Calls from a subframe are refused");
  const url = sender.frameUrl === undefined ? undefined : URL.parse(sender.frameUrl);
  if (url?.protocol !== `${APP_SCHEME}:` || url.host !== APP_HOST) {
    throw new BridgeRefusal("Only the top-level app:// document may use the desktop bridge");
  }
}

