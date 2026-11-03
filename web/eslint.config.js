// Lint for the web workspace. scripts/eslint-boundaries.ts holds the package
// import rules of CONTRACTS.md §2.2; the rest is the stock TypeScript and React set.

import js from "@eslint/js";
import { defineConfig, globalIgnores } from "eslint/config";
import reactHooks from "eslint-plugin-react-hooks";
import tseslint from "typescript-eslint";
import boundaries from "./scripts/eslint-boundaries.ts";

const transportGlobals = ["WebSocket", "EventSource", "XMLHttpRequest"].map((name) => ({
  name,
  message: "Only @aether-ui/core talks to transports (CONTRACTS.md §2.2); send through useCommand.",
}));

export default defineConfig([
  globalIgnores([
    "**/node_modules/",
    "**/dist/",
    "packages/protocol/src/aether.ts",
    "packages/protocol/src/archon.ts",
    "packages/protocol/src/index.ts",
  ]),
  js.configs.recommended,
  tseslint.configs.strict,
  reactHooks.configs.flat.recommended,
  {
    files: ["packages/**/*.{ts,tsx,js,jsx}"],
    plugins: { aether: boundaries },
    rules: { "aether/import-boundaries": ["error", { root: import.meta.dirname }] },
  },
  {
    files: ["packages/**/*.{ts,tsx,js,jsx}"],
    ignores: ["packages/core/**"],
    rules: {
      "no-restricted-globals": ["error", ...transportGlobals],
      "no-restricted-properties": ["error",
        { object: "window", property: "desktop", message: "Only @aether-ui/core reaches the desktop bridge." },
        { object: "globalThis", property: "desktop", message: "Only @aether-ui/core reaches the desktop bridge." },
        { object: "window", property: "WebSocket", message: "Only @aether-ui/core talks to transports." }],
    },
  },
]);
