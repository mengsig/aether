// The shell's own screens: pairing is A2's flow behind A3's screen (CONTRACTS.md §2.3).

import type { RouteDef } from "@aether-ui/protocol";

export const appRoutes: RouteDef[] = [
  { path: "#/pair/:daemon", owner: "A3", load: () => import("./pair-screen.tsx") },
];
