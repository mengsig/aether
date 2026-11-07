import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { spawnLogged } from "./process.ts";

describe("spawnLogged", () => {
  it("keeps the real cause when the binary is missing", () => {
    const dir = mkdtempSync(path.join(tmpdir(), "spawn-"));
    const options = { env: {}, cwd: dir, log: path.join(dir, "log") };
    let thrown: unknown;
    try {
      spawnLogged(path.join(dir, "absent"), [], options);
    } catch (error) {
      thrown = error;
    }
    expect((thrown as Error).message).toMatch(/did not start: ENOENT/);
    expect(((thrown as Error).cause as NodeJS.ErrnoException).code).toBe("ENOENT");
  });
});
