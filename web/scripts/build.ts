// Builds the browser entry with Vite once apps/web (task A3) provides its
// vite.config.ts; until then there is nothing to bundle. --config overrides it.

import { existsSync } from "node:fs";
import path from "node:path";
import { parseArgs } from "node:util";
import { build } from "vite";
import { WEB_ROOT } from "./aliases.ts";

const { values } = parseArgs({
  options: { config: { type: "string", default: path.join(WEB_ROOT, "apps", "web", "vite.config.ts") } },
});
const config = path.resolve(values.config);
if (!existsSync(config)) {
  console.log(`build: ${path.relative(WEB_ROOT, config)} does not exist yet, so there is no bundle to build`);
} else {
  await build({ configFile: config, root: path.dirname(config) });
}
