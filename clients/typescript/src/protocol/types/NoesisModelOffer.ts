// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  NoesisBilling,
} from "../../protocol.ts";

/**
 * A model as Noesis sees it: how it is paid for and whether a step on it
 * could run now (an engine detected and signed in, an API provider keyed).
 */
export interface NoesisModelOffer {
  "billing": NoesisBilling;
  "id": string;
  "ready": boolean;
}
