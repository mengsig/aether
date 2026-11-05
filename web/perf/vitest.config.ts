// The perf scenarios (perf/scenarios): one file at a time in one forked worker, so no two
// scenarios share the CPU, with --expose-gc for the heap measures.

import { defineConfig } from "vitest/config";
import { packageAliases, WEB_ROOT } from "../scripts/aliases.ts";

export default defineConfig({
  root: WEB_ROOT,
  resolve: { alias: packageAliases },
  test: {
    include: ["perf/scenarios/*.perf.tsx"],
    environment: "happy-dom",
    pool: "forks",
    fileParallelism: false,
    maxWorkers: 1,
    execArgv: ["--expose-gc"],
    testTimeout: 300_000,
    hookTimeout: 60_000,
  },
});
