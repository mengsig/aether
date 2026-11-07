// JSON Schema checks for everything the renderer sends: client frames against the daemons'
// generated schemas, one compiled variant per command, and the bridge's own arguments.

import { readFileSync } from "node:fs";
import Ajv2020, { type ValidateFunction } from "ajv/dist/2020.js";
import type { DaemonKind } from "./classes.ts";

/** A daemon protocol schema as `clients/protocol.schema.json` and the vendored Archon copy hold it. */
export interface ProtocolSchema {
  readonly protocol_version: number;
  readonly $defs: Readonly<Record<string, unknown>>;
}

// The generated schemas name integer widths and timestamps as formats; ranges are in the schema.
const FORMATS = ["uint8", "uint16", "uint32", "uint64", "int32", "int64", "double", "float", "date-time", "uint"];

export function newAjv(): Ajv2020 {
  const ajv = new Ajv2020({ strict: false, allErrors: false, validateSchema: false });
  for (const format of FORMATS) ajv.addFormat(format, true);
  return ajv;
}

export function readProtocolSchema(file: string): ProtocolSchema {
  const parsed: unknown = JSON.parse(readFileSync(file, "utf8"));
  if (typeof parsed !== "object" || parsed === null) throw new Error(`${file} is not a JSON object`);
  const schema = parsed as Partial<ProtocolSchema>;
  if (!Number.isInteger(schema.protocol_version)) throw new Error(`${file} names no protocol_version`);
  if (typeof schema.$defs !== "object" || schema.$defs === null) throw new Error(`${file} has no $defs`);
  if (!Array.isArray((schema.$defs.ClientFrame as { oneOf?: unknown } | undefined)?.oneOf)) {
    throw new Error(`${file} has no ClientFrame variants`);
  }
  return schema as ProtocolSchema;
}

function errorText(validate: ValidateFunction): string {
  const first = validate.errors?.[0];
  if (first === undefined) return "does not match its schema";
  return `${first.instancePath || "/"} ${first.message ?? "is invalid"}`;
}

/** The client frames of one daemon, by command, each compiled on first use. */
export class FrameSchema {
  readonly protocolVersion: number;
  private readonly variants = new Map<string, Record<string, unknown>>();
  private readonly compiled = new Map<string, ValidateFunction>();
  private readonly ajv: Ajv2020;
  private readonly defs: Readonly<Record<string, unknown>>;

  constructor(schema: ProtocolSchema, ajv: Ajv2020) {
    this.protocolVersion = schema.protocol_version;
    this.ajv = ajv;
    this.defs = schema.$defs;
    const variants = (schema.$defs.ClientFrame as { oneOf: Array<Record<string, unknown>> }).oneOf;
    for (const variant of variants) {
      const cmd = (variant.properties as { cmd?: { const?: unknown } } | undefined)?.cmd?.const;
      if (typeof cmd !== "string") throw new Error("every ClientFrame variant names its cmd");
      if (this.variants.has(cmd)) throw new Error(`ClientFrame names ${cmd} twice`);
      this.variants.set(cmd, variant);
    }
  }

  commands(): string[] {
    return [...this.variants.keys()];
  }

  /** Why `frame` is not a valid client frame, or undefined when it is. */
  check(frame: Readonly<Record<string, unknown>>): string | undefined {
    const cmd = frame.cmd;
    if (typeof cmd !== "string") return "A frame names its command in cmd";
    const variant = this.variants.get(cmd);
    if (variant === undefined) return `Unknown command ${cmd.slice(0, 64)}`;
    let validate = this.compiled.get(cmd);
    if (validate === undefined) {
      validate = this.ajv.compile({ ...variant, $defs: this.defs });
      this.compiled.set(cmd, validate);
    }
    return validate(frame) ? undefined : `${cmd}: ${errorText(validate)}`;
  }
}

/** A connection id main handed out from `connect`: only local connections exist. */
export const CONN = { type: "string", pattern: "^local:[0-9a-f-]{36}$" };
const MEMORY_ID = { type: "string", pattern: "^mem_[0-9A-HJKMNP-TV-Z]{26}$" };
const DEVICE_ID = { type: "string", pattern: "^dev_[0-9A-HJKMNP-TV-Z]{26}$" };
const ITEM_ID = { type: "string", pattern: "^qi_[0-9A-HJKMNP-TV-Z]{26}$" };
const USD = { type: "number", minimum: 0, maximum: 1e9 };

/** The host-only Noesis keys of CONTRACTS.md §6.1 a `noesis.configure` patch may name. */
export const NOESIS_PATCH = {
  type: "object", additionalProperties: false, minProperties: 1,
  properties: {
    "noesis.enabled": { type: "boolean" },
    "noesis.budget.interactive_usd_daily": USD,
    "noesis.budget.background_usd_daily": USD,
    "noesis.budget.background_tokens_daily": { type: "integer", minimum: 0, maximum: Number.MAX_SAFE_INTEGER },
    "noesis.billing.allow_api": { type: "boolean" },
    "noesis.prompts.overrides": { type: "boolean" },
  },
};

function action(kinds: readonly string[], properties: Record<string, unknown>, extra: Record<string, unknown> = {}) {
  return {
    type: "object", additionalProperties: false,
    required: ["kind", "conn", ...Object.keys(properties)],
    properties: { kind: { enum: kinds }, conn: CONN, ...properties },
    ...extra,
  };
}

const OPERATOR_CHANGE = {
  type: "object", additionalProperties: false, required: ["kind", "conn", "request"],
  properties: {
    kind: { const: "archon.operator_change" }, conn: CONN,
    request: { enum: ["merge", "unblock", "settings"] }, item: ITEM_ID,
    patch: { $ref: "#/$defs/OperatorSettings", minProperties: 1 },
  },
  oneOf: [
    { properties: { request: { enum: ["merge", "unblock"] } }, required: ["item"], not: { required: ["patch"] } },
    { properties: { request: { const: "settings" } }, required: ["patch"], not: { required: ["item"] } },
  ],
};

/** The closed `PrivilegedAction` union of CONTRACTS.md §2.4 as a schema; Archon's settings type is referenced. */
export function privilegedSchema(archon: ProtocolSchema): Record<string, unknown> {
  return {
    $defs: archon.$defs,
    oneOf: [
      action(["memory.accept", "memory.reject"], { memory: MEMORY_ID }),
      action(["device.pair"], { name: { type: "string", minLength: 1 }, role: { enum: ["operator", "observer"] } }),
      action(["device.revoke"], { device: DEVICE_ID }),
      action(["remote.enable", "remote.disable"], {}),
      action(["remote.mount"], { prefix: { const: "/archon" }, target_port: { type: "integer", minimum: 1, maximum: 65535 } }),
      action(["noesis.configure"], { patch: NOESIS_PATCH }),
      OPERATOR_CHANGE,
      action(["engine.login"], { engine: { enum: ["claude", "codex"] } }),
    ],
  };
}

export interface Schemas {
  readonly frames: Readonly<Record<DaemonKind, FrameSchema>>;
  readonly privileged: ValidateFunction;
  readonly ajv: Ajv2020;
}

export function compileSchemas(aether: ProtocolSchema, archon: ProtocolSchema): Schemas {
  const ajv = newAjv();
  return {
    frames: { aether: new FrameSchema(aether, ajv), archon: new FrameSchema(archon, ajv) },
    privileged: ajv.compile(privilegedSchema(archon)),
    ajv,
  };
}

export function schemaError(validate: ValidateFunction): string {
  return errorText(validate);
}
