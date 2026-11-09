import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { createRequire } from "node:module";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const harness = fileURLToPath(new URL("./electron/window-recovery.ts", import.meta.url));

for (const [mode, description] of [
  ["recovery", "the main window recovers without disturbing other windows or services"],
  ["quit", "the app quits only after all windows close"],
] as const) test(description, { timeout: 90_000 }, async () => {
  const dir = mkdtempSync(path.join(os.tmpdir(), "window-recovery-"));
  const electron = createRequire(import.meta.url)("electron") as string;
  assert.equal(typeof electron, "string");
  const env: NodeJS.ProcessEnv = { ...process.env, HOME: dir, XDG_CONFIG_HOME: dir, DISPLAY: "", WAYLAND_DISPLAY: "" };
  delete env.ELECTRON_RUN_AS_NODE;
  try {
    const child = spawn(electron, ["--ozone-platform=headless", harness, mode], { env });
    let output = "";
    child.stdout.on("data", (chunk: Buffer) => { output += chunk.toString(); });
    child.stderr.on("data", (chunk: Buffer) => { output += chunk.toString(); });
    const timer = setTimeout(() => child.kill("SIGKILL"), 80_000);
    const code = await new Promise<number | null>((resolve, reject) => {
      child.once("error", reject);
      child.once("close", resolve);
    }).finally(() => clearTimeout(timer));
    assert.equal(code, 0, output);
    assert.match(output, mode === "quit" ? /WINDOW-QUIT-OK/ : /WINDOW-RECOVERY-OK/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
