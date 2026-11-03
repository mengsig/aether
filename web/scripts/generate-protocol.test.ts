import { spawnSync } from "node:child_process";
import { copyFileSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import ts from "typescript";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { generateProtocol, type SchemaSource } from "./protocol/generate.ts";

const WEB = path.resolve(import.meta.dirname, "..");
const CLI = path.join(WEB, "scripts", "generate-protocol.ts");
const AETHER_SCHEMA = path.join(WEB, "..", "clients", "protocol.schema.json");
const ARCHON_FIXTURE = path.join(WEB, "scripts", "fixtures", "archon.protocol.schema.json");
const APP = path.join(WEB, "packages", "protocol", "src", "app.ts");

const aether: SchemaSource = { module: "aether", label: "clients/protocol.schema.json", text: readFileSync(AETHER_SCHEMA, "utf8") };
const archon: SchemaSource = { module: "archon", label: "fixture.json", text: readFileSync(ARCHON_FIXTURE, "utf8") };

function runCli(...args: string[]) {
  const result = spawnSync(process.execPath, [CLI, ...args], { encoding: "utf8", timeout: 30_000 });
  expect(result.error).toBeUndefined();
  return result;
}

function snapshot(dir: string): Record<string, string> {
  return Object.fromEntries(readdirSync(dir).sort().map((name) => [name, readFileSync(path.join(dir, name), "utf8")]));
}

function archonWith(change: (schema: Record<string, unknown>) => void): SchemaSource {
  const schema = JSON.parse(archon.text) as Record<string, unknown>;
  change(schema);
  return { ...archon, text: JSON.stringify(schema) };
}

function typeErrors(dir: string): string[] {
  const config = ts.readConfigFile(path.join(WEB, "tsconfig.json"), (file) => ts.sys.readFile(file));
  const parsed = ts.parseJsonConfigFileContent(config.config, ts.sys, WEB);
  expect(parsed.errors).toEqual([]);
  const program = ts.createProgram([path.join(dir, "index.ts")], parsed.options);
  return ts.getPreEmitDiagnostics(program).map((d) => ts.flattenDiagnosticMessageText(d.messageText, "\n"));
}

describe("generateProtocol", () => {
  it("gives the same bytes on a second run", () => {
    const first = generateProtocol([aether, archon]);
    const second = generateProtocol([archon, aether]);
    expect([...first.keys()].sort()).toEqual(["aether.ts", "archon.ts", "index.ts"]);
    expect(second).toEqual(first);
  });

  it("renders the schema shapes of the Archon fixture", () => {
    const module = generateProtocol([aether, archon]).get("archon.ts") ?? "";
    expect(module).toContain("export const PROTOCOL_VERSION = 24;\nexport const MIN_PROTOCOL_VERSION = 23;");
    expect(module).toContain('export type ClientFrame = { "req": number } & Command;');
    expect(module).toContain("export type ChangeSubject = ChangeSubjectItem | ChangeSubjectDependency;");
    expect(module).toContain('  "stage"?: string | null;');
    expect(module).toContain('  "device"?: DeviceInfo | null;');
    expect(module).toContain('  "data": Record<string, string>;');
    expect(module).toContain("export function status(): StatusCommand {");
    expect(module).toContain('return { cmd: "subscribe_changes", ...args };');
    expect(module).toContain("A comment end *\\/ in a description");
  });

  it("leaves the Archon module out of the index without its schema", () => {
    const files = generateProtocol([aether]);
    expect([...files.keys()].sort()).toEqual(["aether.ts", "index.ts"]);
    expect(files.get("index.ts")).not.toContain("archon");
  });

  it("refuses schemas it cannot render faithfully", () => {
    expect(() => generateProtocol([archon])).toThrow(/Aether schema is required/);
    const noVersion = archonWith((schema) => delete schema["protocol_version"]);
    expect(() => generateProtocol([aether, noVersion])).toThrow(/cannot generate archon\.ts/);
    const broken = { ...archon, text: "{" };
    expect(() => generateProtocol([aether, broken])).toThrow(/cannot generate archon\.ts from fixture\.json/);
    const reserved = archonWith((schema) => {
      const frame = (schema["$defs"] as Record<string, { oneOf: Array<{ properties: { cmd: { const: string } } }> }>)["ClientFrame"];
      const status = frame?.oneOf.find((branch) => branch.properties.cmd.const === "status");
      if (status !== undefined) status.properties.cmd.const = "delete";
    });
    expect(() => generateProtocol([aether, reserved])).toThrow(/cannot generate archon\.ts/);
    const clash = archonWith((schema) => {
      (schema["$defs"] as Record<string, unknown>)["StatusCommand"] = { type: "string" };
    });
    expect(() => generateProtocol([aether, clash])).toThrowError(expect.objectContaining({
      cause: expect.objectContaining({ message: "two generated declarations are named StatusCommand" }),
    }));
  });
});

describe("generate-protocol CLI", () => {
  let out = "";
  beforeEach(() => {
    out = mkdtempSync(path.join(os.tmpdir(), "aether-ui-protocol-"));
    copyFileSync(APP, path.join(out, "app.ts"));
  });
  afterEach(() => rmSync(out, { recursive: true, force: true }));

  it("finds the committed package up to date", () => {
    const result = runCli("--check");
    expect(result.stderr).toBe("");
    expect(result.status).toBe(0);
  });

  it("changes nothing on a second run, and drops the Archon module with its schema", () => {
    expect(runCli("--archon-schema", ARCHON_FIXTURE, "--out-dir", out).status).toBe(0);
    const first = snapshot(out);
    const again = runCli("--archon-schema", ARCHON_FIXTURE, "--out-dir", out);
    expect(again.stdout).toContain("0 file(s) updated");
    expect(snapshot(out)).toEqual(first);
    expect(runCli("--check", "--archon-schema", ARCHON_FIXTURE, "--out-dir", out).status).toBe(0);

    const missing = path.join(out, "absent.json");
    expect(runCli("--check", "--archon-schema", missing, "--out-dir", out).status).toBe(1);
    expect(runCli("--archon-schema", missing, "--out-dir", out).status).toBe(0);
    expect(Object.keys(snapshot(out))).toEqual(["aether.ts", "app.ts", "index.ts"]);
  });

  it("typechecks the Archon module beside app.ts", () => {
    expect(runCli("--archon-schema", ARCHON_FIXTURE, "--out-dir", out).status).toBe(0);
    expect(typeErrors(out)).toEqual([]);
  }, 60_000);

  it("never removes a file it did not write", () => {
    writeFileSync(path.join(out, "archon.ts"), "export const HAND_WRITTEN = true;\n");
    const result = runCli("--archon-schema", path.join(out, "absent.json"), "--out-dir", out);
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain("was not written by this generator");
    expect(readFileSync(path.join(out, "archon.ts"), "utf8")).toContain("HAND_WRITTEN");
  });
});
