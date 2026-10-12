// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  DeviceProof,
} from "../../protocol.ts";


export interface HelloCommand {
  "cmd": "hello";
  "client_name": string;
  "client_version": string;
  "device"?: DeviceProof | null;
  "features"?: Array<string>;
  "protocol_version": number;
  "token"?: string | null;
}

export function hello(args: Omit<HelloCommand, "cmd">): HelloCommand {
  return { cmd: "hello", ...args };
}
