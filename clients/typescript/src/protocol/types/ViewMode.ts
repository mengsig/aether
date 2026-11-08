// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  ViewModeCategory,
  ViewModeDensity,
  ViewModeScope,
} from "../../protocol.ts";

export interface ViewMode {
  "builtin": boolean;
  "density": ViewModeDensity;
  "description"?: string | null;
  "label"?: string | null;
  "name": string;
  "scope"?: ViewModeScope | null;
  "show": Array<ViewModeCategory>;
}
