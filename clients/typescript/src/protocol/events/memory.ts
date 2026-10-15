// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  MemoryCandidate,
  MemoryId,
} from "../../protocol.ts";


export interface EventKindMemoryCandidate {
  "candidate": MemoryCandidate;
  "type": "memory.candidate";
}

export interface EventKindMemoryAccepted {
  "memory": MemoryId;
  "reason"?: string | null;
  "type": "memory.accepted";
}

export interface EventKindMemoryRejected {
  "reason": string;
  "type": "memory.rejected";
}

/**
 * Curated out by a person. The record stays; it is never retrieved again.
 */
export interface EventKindMemoryForgotten {
  "memory": MemoryId;
  "reason"?: string | null;
  "type": "memory.forgotten";
}

/**
 * Put back after a `memory.forgotten`, so a mis-keyed curation has a way
 * back that leaves the same trail everything else does.
 */
export interface EventKindMemoryRestored {
  "memory": MemoryId;
  "type": "memory.restored";
}

/**
 * Pinned in front of the model, or unpinned.
 */
export interface EventKindMemoryPinned {
  "memory": MemoryId;
  "pinned": boolean;
  "type": "memory.pinned";
}

export interface EventKindMemorySuperseded {
  "new": MemoryId;
  "old": MemoryId;
  "reason"?: string | null;
  "type": "memory.superseded";
}
