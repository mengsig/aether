// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  DeviceId,
} from "../../protocol.ts";

/**
 * Server-observed connection identity, never a bearer token or client-supplied name.
 * Tagged by `transport`.
 */
export interface OperatorOriginLocalSocket {
  "connection": string;
  "transport": "local_socket";
}

export interface OperatorOriginTcp {
  "connection": string;
  "peer": string;
  "transport": "tcp";
}

/**
 * A paired remote device, named as the daemon enrolled it.
 */
export interface OperatorOriginDevice {
  "connection": string;
  "device": DeviceId;
  "name": string;
  "transport": "device";
}

export type OperatorOrigin =
    OperatorOriginLocalSocket |
    OperatorOriginTcp |
    OperatorOriginDevice
;
