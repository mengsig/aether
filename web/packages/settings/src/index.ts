/// <reference types="vite/client" />
// Registers every `src/<dir>/routes.ts` of this package (CONTRACTS.md §2.1), so a
// feature task adds a directory and never edits this file. A routes.ts default-
// exports its RouteDef[] and may export `actions`, its EntityAction[].

import { registerEntityAction, registerRoutes } from "@aether-ui/core";
import type { EntityAction, RouteDef } from "@aether-ui/protocol";

interface RoutesModule {
  readonly default: readonly RouteDef[];
  readonly actions?: readonly EntityAction[];
}

const modules = import.meta.glob<RoutesModule>("./*/routes.ts", { eager: true });

for (const [file, module] of Object.entries(modules)) {
  if (!Array.isArray(module.default)) throw new TypeError(`${file} must default-export its RouteDef[]`);
  registerRoutes([...module.default]);
  for (const action of module.actions ?? []) registerEntityAction(action);
}

export const registeredModules: readonly string[] = Object.keys(modules).sort();

// What the app shell composes from setup: the overview's home and the Archon tools notice.
export const loadWelcome = () => import("./setup/Welcome.tsx");
export const loadArchonToolsNotice = () => import("./setup/ConnectArchon.tsx");
