// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  ViewMode,
  ViewModeDiagnostic,
} from "../../protocol.ts";

export interface ViewModes {
  "diagnostics": Array<ViewModeDiagnostic>;
  "modes": Array<ViewMode>;
}
