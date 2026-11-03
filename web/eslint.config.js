// Lint for the web workspace. scripts/eslint-boundaries.ts holds the package
// import rules of CONTRACTS.md §2.2; the rest is the stock TypeScript and React set.

import js from "@eslint/js";
import { defineConfig, globalIgnores } from "eslint/config";
import reactHooks from "eslint-plugin-react-hooks";
import tseslint from "typescript-eslint";
import boundaries from "./scripts/eslint-boundaries.ts";

const TRANSPORTS = ["WebSocket", "EventSource", "XMLHttpRequest", "fetch"];
const TRANSPORT_MESSAGE = "Only @aether-ui/core talks to transports (CONTRACTS.md §2.2); send through useCommand.";
const BRIDGE_MESSAGE = "Only @aether-ui/core reaches the desktop bridge (CONTRACTS.md §2.2).";
const GLOBAL_OBJECTS = ["window", "globalThis", "self"];

const transportGlobals = TRANSPORTS.map((name) => ({ name, message: TRANSPORT_MESSAGE }));
const qualifiedGlobals = GLOBAL_OBJECTS.flatMap((object) => [
  ...TRANSPORTS.map((property) => ({ object, property, message: TRANSPORT_MESSAGE })),
  { object, property: "desktop", message: BRIDGE_MESSAGE },
]);

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
      "no-restricted-properties": ["error", ...qualifiedGlobals],
    },
  },
]);
