// Credentials stay in main: nothing the bridge carries across IPC is typed as, or named like, a
// credential, and a daemon's socket path never reaches the page.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { Bridge, type BridgeHost } from "../../src/bridge/bridge.ts";
import { DESKTOP_ROOT, WEB_ROOT } from "../../src/main/paths.ts";
import { APP_SENDER, IDENTITY, VALIDATOR } from "./support.ts";

const CREDENTIAL = /token|passw|secret|credential|cookie|api[_-]?key|bearer|authori[sz]ation|private[_-]?key|session[_-]?key/i;

/** The exported declarations named `names` from a TypeScript source, each up to the next export. */
function declarations(source: string, names: readonly string[]): string {
  return names.map((name) => {
    const start = source.search(new RegExp(`export (interface|type) ${name}\\b`));
    assert.ok(start !== -1, `${name} is declared`);
    const end = source.indexOf("\nexport ", start + 1);
    return source.slice(start, end === -1 ? undefined : end);
  }).join("\n");
}

/** The lines of `text` without comments, which may say what does not cross. */
function code(text: string): string[] {
  return text.replace(/\/\*[\s\S]*?\*\//g, "").split("\n").map((line) => line.replace(/\/\/.*$/, ""));
}

test("the bridge types and the preload carry no credential", () => {
  const contract = readFileSync(path.join(WEB_ROOT, "packages/protocol/src/app.ts"), "utf8");
  const bridgeTypes = declarations(contract, ["DesktopBridge", "LocalDaemon", "PrivilegedAction", "PreferenceKey"]);
  assert.match(bridgeTypes, /privileged\(action: PrivilegedAction\)/, "the declarations were found");
  const preload = readFileSync(path.join(DESKTOP_ROOT, "src/preload/index.cjs"), "utf8");
  const validate = readFileSync(path.join(DESKTOP_ROOT, "src/security/validate.ts"), "utf8");
  const calls = validate.slice(validate.indexOf("export type ValidCall"), validate.indexOf("export const BRIDGE_METHODS"));
  for (const [name, text] of [["bridge types", bridgeTypes], ["preload", preload], ["IPC arguments", calls]] as const) {
    assert.ok(text.length > 200, `${name} is not empty`);
    assert.deepEqual(code(text).filter((line) => CREDENTIAL.test(line)), [], name);
  }
});

test("discover and diagnostics never return a socket path", async () => {
  const daemon = { id: "aether:local", ...IDENTITY, reachable: true };
  const host = {
    findDaemons: async () => [daemon], versions: { app: "0.0.0" }, logPath: "/tmp/log",
  } as unknown as BridgeHost;
  const bridge = new Bridge(VALIDATOR, host);
  const discovered = await bridge.handle(APP_SENDER, 1, "discover", []);
  assert.deepEqual(discovered, [{ id: "aether:local", kind: "aether", label: "aetherd", reachable: true }]);
  const diagnostics = await bridge.handle(APP_SENDER, 1, "diagnostics", []);
  assert.ok(!JSON.stringify(diagnostics).includes(IDENTITY.socket));
});
