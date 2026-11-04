import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

const BUILD = path.resolve(import.meta.dirname, "build.ts");

function runBuild(config: string) {
  const result = spawnSync(process.execPath, [BUILD, "--config", config], { encoding: "utf8", timeout: 60_000 });
  expect(result.error).toBeUndefined();
  return result;
}

describe("build script", () => {
  let app = "";
  beforeEach(() => {
    app = mkdtempSync(path.join(os.tmpdir(), "aether-ui-build-"));
  });
  afterEach(() => rmSync(app, { recursive: true, force: true }));

  it("says there is nothing to build without an app config", () => {
    const result = runBuild(path.join(app, "vite.config.ts"));
    expect(result.status).toBe(0);
    expect(result.stdout).toContain("does not exist yet");
  });

  it("bundles an app with Vite when its config exists", () => {
    writeFileSync(path.join(app, "index.html"), '<!doctype html><script type="module" src="./main.ts"></script>\n');
    writeFileSync(path.join(app, "main.ts"), 'document.title = "built by vite";\n');
    writeFileSync(path.join(app, "vite.config.ts"), 'export default { build: { outDir: "out" }, logLevel: "silent" };\n');
    const result = runBuild(path.join(app, "vite.config.ts"));
    expect(result.stderr).toBe("");
    expect(result.status).toBe(0);
    expect(existsSync(path.join(app, "out", "index.html"))).toBe(true);
    expect(readFileSync(path.join(app, "out", "index.html"), "utf8")).toContain("/assets/");
  }, 60_000);

  it("fails when Vite cannot build the app", () => {
    writeFileSync(path.join(app, "index.html"), '<!doctype html><script type="module" src="./missing.ts"></script>\n');
    writeFileSync(path.join(app, "vite.config.ts"), 'export default { logLevel: "silent" };\n');
    expect(runBuild(path.join(app, "vite.config.ts")).status).not.toBe(0);
  }, 60_000);
});
