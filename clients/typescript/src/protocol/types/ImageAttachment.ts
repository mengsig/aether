// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.


/**
 * One image as a client sends it with a `send_message` command.
 */
export interface ImageAttachment {
  "data": string;
  "media_type": string;
  "name"?: string | null;
}
