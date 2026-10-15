// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  Usage,
} from "../../protocol.ts";

/**
 * Transient streaming signal. Not persisted; durable events reconcile it.
 * Tagged by `delta`.
 */
export interface DeltaTurnStarted {
  "delta": "turn_started";
  "turn": number;
}

export interface DeltaText {
  "delta": "text";
  "text": string;
}

export interface DeltaThinking {
  "delta": "thinking";
  "text": string;
}

export interface DeltaToolInput {
  "delta": "tool_input";
  "name": string;
  "partial_json": string;
}

/**
 * Absolute in-memory figures. Replace prior totals; never add these to durable usage.
 */
export interface DeltaAccounting {
  "context_tokens"?: number | null;
  "delta": "accounting";
  "total": Usage;
  "turn_usage": Usage;
}

export interface DeltaTurnFinished {
  "delta": "turn_finished";
}

export type Delta =
    DeltaTurnStarted |
    DeltaText |
    DeltaThinking |
    DeltaToolInput |
    DeltaAccounting |
    DeltaTurnFinished
;
