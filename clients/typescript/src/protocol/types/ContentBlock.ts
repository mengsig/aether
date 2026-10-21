// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AttachmentId,
  ToolResultBlock,
  compact,
} from "../../protocol.ts";

/**
 * A block inside a message.
 * Tagged by `type`.
 */
export interface ContentBlockText {
  "text": string;
  "type": "text";
}

export interface ContentBlockThinking {
  "redacted"?: boolean;
  "signature"?: string | null;
  "text": string;
  "type": "thinking";
}

export interface ContentBlockToolUse {
  "id": string;
  "input": unknown;
  "name": string;
  "type": "tool_use";
}

export interface ContentBlockToolResult {
  "content": Array<ToolResultBlock>;
  "is_error"?: boolean;
  "tool_use_id": string;
  "type": "tool_result";
}

export interface ContentBlockImage {
  "data": string;
  "media_type": string;
  "type": "image";
}

/**
 * An image a person sent with a message, kept by the daemon. Durable
 * state carries this reference; the bytes are read back only to build
 * a model request.
 */
export interface ContentBlockImageRef {
  "id": AttachmentId;
  "media_type": string;
  "name"?: string | null;
  "size_bytes": number;
  "type": "image_ref";
}

/**
 * A server-side compaction summary (Anthropic `compact-2026-01-12`). It
 * must be replayed verbatim: the API drops everything before it.
 */
export interface ContentBlockCompaction {
  "content": string;
  "type": "compaction";
}

export type ContentBlock =
    ContentBlockText |
    ContentBlockThinking |
    ContentBlockToolUse |
    ContentBlockToolResult |
    ContentBlockImage |
    ContentBlockImageRef |
    ContentBlockCompaction
;
