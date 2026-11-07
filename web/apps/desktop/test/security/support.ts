// Shared by the unit tests: the real compiled schemas, a Validator over them, senders and a
// connection table whose two connections are one Aether and one Archon daemon.

import { AETHER_SCHEMA, ARCHON_SCHEMA } from "../../src/main/paths.ts";
import { compileSchemas, readProtocolSchema, type Schemas } from "../../src/security/schema.ts";
import { Validator, type Connections, type Sender } from "../../src/security/validate.ts";

export const AETHER_CONN = "local:00000000-0000-4000-8000-000000000001";
export const ARCHON_CONN = "local:00000000-0000-4000-8000-000000000002";
export const MEMORY = "mem_01JAAAAAAAAAAAAAAAAAAAAAAA";
export const DEVICE = "dev_01JAAAAAAAAAAAAAAAAAAAAAAA";
export const ITEM = "qi_01JAAAAAAAAAAAAAAAAAAAAAAA";

export const SCHEMAS: Schemas = compileSchemas(readProtocolSchema(AETHER_SCHEMA), readProtocolSchema(ARCHON_SCHEMA));
export const VALIDATOR = new Validator(SCHEMAS);

export const APP_SENDER: Sender = { isAppWindow: true, isTopFrame: true, frameUrl: "app://aether/index.html#/agents" };

export const CONNECTIONS: Connections = {
  kindOf: (conn) => (conn === AETHER_CONN ? "aether" : conn === ARCHON_CONN ? "archon" : undefined),
  protocolOf: (conn) => (conn === ARCHON_CONN ? 26 : undefined),
};

export const IDENTITY = { kind: "aether", label: "aetherd", socket: "/run/user/1000/aether/aetherd.sock" } as const;
