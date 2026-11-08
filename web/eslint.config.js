// Lint for the web workspace. scripts/eslint-boundaries.ts holds the package import rules of
// CONTRACTS.md §2.2, scripts/eslint-tokens.ts the §4 token rule; the rest is the stock set.

import js from "@eslint/js";
import { defineConfig, globalIgnores } from "eslint/config";
import reactHooks from "eslint-plugin-react-hooks";
import tseslint from "typescript-eslint";
import boundaries from "./scripts/eslint-boundaries.ts";
import tokens from "./scripts/eslint-tokens.ts";

const TRANSPORTS = ["WebSocket", "EventSource", "XMLHttpRequest", "fetch"];
const TRANSPORT_MESSAGE = "Only @aether-ui/core talks to transports (CONTRACTS.md §2.2); send through useCommand.";
const BRIDGE_MESSAGE = "Only @aether-ui/core reaches the desktop bridge (CONTRACTS.md §2.2).";
const GLOBAL_OBJECTS = ["window", "globalThis", "self"];
// Every module kind ESLint, TypeScript or Vite would accept inside a package.
const PACKAGE_SOURCES = ["packages/**/*.{ts,tsx,mts,cts,js,jsx,mjs,cjs}"];
const MODULES = "{ts,tsx,mts,cts,js,jsx,mjs,cjs}";
// Everything that draws: the UI kit, the feature packages and the browser entry.
const COMPONENT_SOURCES = ["ui", "aether", "archon", "github", "home", "noesis", "settings"]
  .map((name) => `packages/${name}/src/**/*.${MODULES}`)
  .concat(`apps/web/src/**/*.${MODULES}`);

const transportGlobals = TRANSPORTS.map((name) => ({ name, message: TRANSPORT_MESSAGE }));
const qualifiedGlobals = GLOBAL_OBJECTS.flatMap((object) => [
  ...TRANSPORTS.map((property) => ({ object, property, message: TRANSPORT_MESSAGE })),
  { object, property: "desktop", message: BRIDGE_MESSAGE },
]);

export default defineConfig([
  globalIgnores([
    "**/node_modules/",
    "**/dist/",
    "e2e/.out/",
    "packages/protocol/src/aether.ts",
    "packages/protocol/src/archon.ts",
    "packages/protocol/src/index.ts",
  ]),
  js.configs.recommended,
  tseslint.configs.strict,
  reactHooks.configs.flat.recommended,
  {
    files: PACKAGE_SOURCES,
    plugins: { aether: boundaries },
    rules: { "aether/import-boundaries": ["error", { root: import.meta.dirname }] },
  },
  {
    files: PACKAGE_SOURCES,
    ignores: ["packages/core/**"],
    rules: {
      "no-restricted-globals": ["error", ...transportGlobals],
      "no-restricted-properties": ["error", ...qualifiedGlobals],
    },
  },
  {
    files: COMPONENT_SOURCES,
    ignores: ["**/*.test.*"],
    plugins: { tokens },
    rules: { "tokens/design-tokens": "error" },
  },
]);
