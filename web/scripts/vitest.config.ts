import { defineConfig } from "vitest/config";
import { WEB_ROOT, packageAliases } from "./aliases.ts";

export default defineConfig({
  root: WEB_ROOT,
  resolve: { alias: packageAliases },
  test: {
    include: [
      "packages/*/{src,test}/**/*.test.{ts,tsx}",
      "apps/web/{src,test}/**/*.test.{ts,tsx}",
      "scripts/**/*.test.ts",
      "perf/**/*.test.ts",
    ],
    environment: "node",
    testTimeout: 30_000,
  },
});
