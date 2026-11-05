// The wire under the page: a frame for any command, built from the daemons' own protocol schemas,
// and a tap on the page's authenticated WebSockets that sends such frames as the paired device.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import type { Page, WebSocketRoute } from "@playwright/test";

type Node = Readonly<Record<string, unknown>>;
export type Frame = Readonly<Record<string, unknown>> & { readonly cmd: string };
export type Daemon = "aether" | "archon";

const SCHEMAS: Record<Daemon, string> = {
  aether: path.resolve(import.meta.dirname, "..", "..", "..", "..", "clients", "protocol.schema.json"),
  archon: path.resolve(import.meta.dirname, "..", "..", "..", "schemas", "archon.protocol.schema.json"),
};
/** An id pattern such as `^agt_[0-9A-HJKMNP-TV-Z]{26}$`: a prefixed ULID. */
const ULID_ID = /^\^([a-z]+)_\[0-9A-HJKMNP-TV-Z\]\{26\}\$$/;
const MAX_DEPTH = 12;

export class ProtocolSchema {
  readonly #defs: Readonly<Record<string, Node>>;
  readonly #commands: ReadonlyMap<string, Node>;

  constructor(daemon: Daemon) {
    const schema = JSON.parse(readFileSync(SCHEMAS[daemon], "utf8")) as { $defs?: Record<string, Node> };
    assert.ok(schema.$defs?.ClientFrame !== undefined, `${SCHEMAS[daemon]} defines ClientFrame`);
    this.#defs = schema.$defs;
    const variants = (this.#defs.ClientFrame as { oneOf?: Node[] }).oneOf ?? [];
    const commands = new Map<string, Node>();
    for (const variant of variants) {
      const cmd = (variant.properties as Record<string, { const?: unknown }> | undefined)?.cmd?.const;
      if (typeof cmd === "string") commands.set(cmd, variant);
    }
    assert.ok(commands.size > 20, `${daemon}'s ClientFrame names its commands`);
    this.#commands = commands;
  }

  /** The smallest frame of `cmd` the daemon parses: every required field, ids that name nothing. */
  frame(cmd: string, overrides: Readonly<Record<string, unknown>> = {}): Frame {
    const variant = this.#commands.get(cmd);
    assert.ok(variant !== undefined, `the schema has no command ${cmd}`);
    const value = this.#sample(variant, 0) as Record<string, unknown>;
    assert.equal(value.cmd, cmd, "the sample carries its own cmd");
    return { ...value, ...overrides, cmd };
  }

  #resolve(node: Node): Node {
    const ref = node.$ref;
    if (typeof ref !== "string") return node;
    const name = ref.replace("#/$defs/", "");
    const target = this.#defs[name];
    assert.ok(target !== undefined, `the schema defines ${name}`);
    const rest: Record<string, unknown> = { ...node };
    delete rest.$ref;
    const resolved = this.#resolve(target);
    const merged: Record<string, unknown> = { ...resolved, ...rest };
    if (resolved.properties !== undefined || rest.properties !== undefined) {
      merged.properties = { ...(resolved.properties as Node | undefined), ...(rest.properties as Node | undefined) };
      merged.required = [...((resolved.required as string[] | undefined) ?? []), ...((rest.required as string[] | undefined) ?? [])];
    }
    return merged;
  }

  #sample(raw: Node, depth: number): unknown {
    assert.ok(depth < MAX_DEPTH, "a sample frame is shallow");
    const node = this.#resolve(raw);
    if ("const" in node) return node.const;
    if (Array.isArray(node.enum)) return node.enum[0];
    const choices = (node.oneOf ?? node.anyOf) as Node[] | undefined;
    // An object variant over a bare word: `archive {target: {agent}}`, never `all_finished`.
    const choice = choices?.find((variant) => this.#resolve(variant).type === "object") ?? choices?.[0];
    const chosen = choice === undefined ? undefined : this.#sample(choice, depth + 1);
    if (chosen !== undefined && node.properties === undefined) return chosen;
    const types = Array.isArray(node.type) ? node.type as string[] : [node.type ?? (node.properties ? "object" : undefined)];
    const type = types.find((name) => name !== "null");
    const own = this.#scalar(node, type, depth);
    // A tagged variant beside fields of its own, as `pause` is `{cmd}` plus a PauseTarget.
    return chosen !== null && typeof chosen === "object" ? { ...chosen, ...(own as object) } : own;
  }

  #scalar(node: Node, type: unknown, depth: number): unknown {
    switch (type) {
      case "object": {
        const properties = (node.properties ?? {}) as Record<string, Node>;
        const required = new Set((node.required as string[] | undefined) ?? []);
        return Object.fromEntries([...required].map((name) => {
          const property = properties[name];
          assert.ok(property !== undefined, `required ${name} has a schema`);
          return [name, this.#sample(property, depth + 1)];
        }));
      }
      case "string": return sampleString(node);
      case "integer": case "number": return typeof node.minimum === "number" ? Math.max(node.minimum, 1) : 1;
      case "boolean": return false;
      case "array": {
        const count = typeof node.minItems === "number" ? node.minItems : 0;
        return Array.from({ length: count }, () => this.#sample(node.items as Node, depth + 1));
      }
      case undefined: return null;
      default: throw new Error(`no sample for a schema of type ${String(type)}`);
    }
  }
}

function sampleString(node: Node): string {
  if (node.format === "date-time") return new Date(0).toISOString();
  const pattern = typeof node.pattern === "string" ? node.pattern : undefined;
  const id = pattern === undefined ? null : ULID_ID.exec(pattern);
  if (id !== null) return `${id[1]}_${"0".repeat(26)}`;
  const text = "e2e".padEnd(typeof node.minLength === "number" ? node.minLength : 0, "e");
  assert.ok(pattern === undefined || new RegExp(pattern).test(text), `a sample string for the pattern ${pattern}`);
  return text;
}

export interface Answer { readonly ok: unknown; readonly err: unknown }

/** Request ids far above any the app numbers, so a tapped reply never reaches the page. */
const TAP_BASE = 1_000_000;
const ANSWER_MS = 15_000;

/** The page's WebSockets to each daemon, passed through, and able to send frames of their own. */
export class SocketTap {
  readonly #servers = new Map<Daemon, WebSocketRoute>();
  readonly #pending = new Map<number, (answer: Answer) => void>();
  #next = TAP_BASE;

  /** Routes the page's daemon sockets through the tap; call it before the page opens them. */
  static async install(page: Page): Promise<SocketTap> {
    const tap = new SocketTap();
    await page.routeWebSocket((url) => url.pathname === "/ws" || url.pathname === "/archon/ws", (route) => tap.#attach(route));
    return tap;
  }

  #attach(route: WebSocketRoute): void {
    const daemon: Daemon = new URL(route.url()).pathname === "/archon/ws" ? "archon" : "aether";
    const server = route.connectToServer();
    route.onMessage((message) => server.send(message));
    server.onMessage((message) => {
      const text = typeof message === "string" ? message : message.toString("utf8");
      const frame = JSON.parse(text) as { frame?: unknown; id?: unknown; result?: { Ok?: unknown; Err?: unknown } };
      const settle = typeof frame.id === "number" && frame.id >= TAP_BASE ? this.#pending.get(frame.id) : undefined;
      if (settle === undefined) return route.send(message);
      this.#pending.delete(frame.id as number);
      settle({ ok: frame.result?.Ok, err: frame.result?.Err });
    });
    this.#servers.set(daemon, server);
  }

  /** Sends `frame` on the page's live, authenticated socket to `daemon`; resolves with its reply. */
  async send(daemon: Daemon, frame: Frame): Promise<Answer> {
    const server = this.#servers.get(daemon);
    assert.ok(server !== undefined, `the page has a socket to ${daemon}`);
    const req = this.#next++;
    const answered = new Promise<Answer>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#pending.delete(req);
        reject(new Error(`${daemon} did not answer ${frame.cmd} in ${ANSWER_MS} ms`));
      }, ANSWER_MS);
      this.#pending.set(req, (answer) => { clearTimeout(timer); resolve(answer); });
    });
    server.send(JSON.stringify({ ...frame, req }));
    return answered;
  }
}
