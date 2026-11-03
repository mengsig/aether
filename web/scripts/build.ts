// Builds the browser entry into web/dist with Vite once apps/web (task A3)
// provides its vite.config.ts; until then there is nothing to bundle.

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { WEB_ROOT } from "./aliases.ts";

const config = path.join(WEB_ROOT, "apps", "web", "vite.config.ts");
if (!existsSync(config)) {
  console.log("build: apps/web/vite.config.ts does not exist yet, so there is no bundle to build");
} else {
  const vite = createRequire(import.meta.url).resolve("vite/bin/vite.js");
  const result = spawnSync(process.execPath, [vite, "build", "--config", config], { cwd: WEB_ROOT, stdio: "inherit" });
  if (result.error !== undefined) throw new Error("could not start vite", { cause: result.error });
  process.exitCode = result.status ?? 1;
}
