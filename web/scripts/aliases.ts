// Resolves `@aether-ui/<package>` to packages/<package>/src/index.ts, as the
// `paths` of tsconfig.base.json do; Vite and Vitest configs share it.

import path from "node:path";

export const WEB_ROOT = path.resolve(import.meta.dirname, "..");

export const packageAliases = [
  { find: /^@aether-ui\/([a-z][a-z0-9-]*)$/, replacement: path.join(WEB_ROOT, "packages", "$1", "src", "index.ts") },
];
