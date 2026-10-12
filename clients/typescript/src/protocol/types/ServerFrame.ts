// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
  Delta,
  Event,
  InvocationId,
  Result_of_Reply_or_ErrorReply,
  hello,
} from "../../protocol.ts";

/**
 * Daemon to client.
 * Tagged by `frame`.
 */
export interface ServerFrameReply {
  "frame": "reply";
  "id": number;
  "result": Result_of_Reply_or_ErrorReply;
}

export interface ServerFrameEvent {
  "event": Event;
  "frame": "event";
  "sub": number;
}

export interface ServerFrameDelta {
  "agent": AgentId;
  "delta": Delta;
  "frame": "delta";
  "invocation": InvocationId;
  "sub": number;
}

/**
 * The remote carrier speaking first: what a device signs to
 * authenticate its hello. Never sent on the socket or the token listener.
 */
export interface ServerFrameChallenge {
  "frame": "challenge";
  "instance_id": string;
  "nonce": string;
}

export type ServerFrame =
    ServerFrameReply |
    ServerFrameEvent |
    ServerFrameDelta |
    ServerFrameChallenge
;
