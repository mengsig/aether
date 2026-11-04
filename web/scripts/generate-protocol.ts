// Generates @aether-ui/protocol from the daemon schemas.
//
//   node scripts/generate-protocol.ts            write packages/protocol/src
//   node scripts/generate-protocol.ts --check    fail if a generated file is stale
//
// The Archon module is generated only when web/schemas/archon.protocol.schema.json
// exists. --aether-schema, --archon-schema and --out-dir override the paths.

import { existsSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { parseArgs } from "node:util";
import { GENERATED_FILES, GENERATED_MARK, generateProtocol, type SchemaSource } from "./protocol/generate.ts";

const WEB_ROOT = path.resolve(import.meta.dirname, "..");
const REPO_ROOT = path.resolve(WEB_ROOT, "..");

interface Options {
  readonly check: boolean;
  readonly aetherSchema: string;
  readonly archonSchema: string;
  readonly outDir: string;
}

function readOptions(): Options {
  const { values } = parseArgs({
    options: {
      check: { type: "boolean", default: false },
      "aether-schema": { type: "string", default: path.join(REPO_ROOT, "clients", "protocol.schema.json") },
      "archon-schema": { type: "string", default: path.join(WEB_ROOT, "schemas", "archon.protocol.schema.json") },
      "out-dir": { type: "string", default: path.join(WEB_ROOT, "packages", "protocol", "src") },
    },
  });
  return {
    check: values.check,
    aetherSchema: path.resolve(values["aether-schema"]),
    archonSchema: path.resolve(values["archon-schema"]),
    outDir: path.resolve(values["out-dir"]),
  };
}

function source(module: SchemaSource["module"], file: string): SchemaSource {
  const label = path.relative(REPO_ROOT, file).split(path.sep).join("/");
  return { module, label, text: readFileSync(file, "utf8") };
}

function isGenerated(file: string): boolean {
  return readFileSync(file, "utf8").split("\n", 3).some((line) => line.includes(GENERATED_MARK));
}

/** The files that differ from `wanted`; a generated file nobody wants any more counts too. */
function staleFiles(outDir: string, wanted: ReadonlyMap<string, string>): string[] {
  return GENERATED_FILES.filter((name) => {
    const file = path.join(outDir, name);
    const current = existsSync(file) ? readFileSync(file, "utf8") : undefined;
    return current !== wanted.get(name);
  });
}

function write(outDir: string, wanted: ReadonlyMap<string, string>, stale: readonly string[]): void {
  for (const name of stale) {
    const file = path.join(outDir, name);
    const text = wanted.get(name);
    if (text !== undefined) {
      writeFileSync(file, text, "utf8");
      continue;
    }
    if (!isGenerated(file)) throw new Error(`${file} was not written by this generator; remove it by hand`);
    rmSync(file);
  }
}

function main(): number {
  const options = readOptions();
  if (!existsSync(options.outDir)) throw new Error(`output directory ${options.outDir} does not exist`);
  const sources = [source("aether", options.aetherSchema)];
  if (existsSync(options.archonSchema)) sources.push(source("archon", options.archonSchema));
  const wanted = generateProtocol(sources);
  const stale = staleFiles(options.outDir, wanted);
  if (options.check) {
    for (const name of stale) console.error(`${path.join(options.outDir, name)} is stale`);
    if (stale.length > 0) console.error("Run `npm run generate` in web/ and commit the result.");
    return stale.length === 0 ? 0 : 1;
  }
  write(options.outDir, wanted, stale);
  console.log(`protocol: ${sources.map((s) => s.label).join(", ")} -> ${stale.length} file(s) updated`);
  return 0;
}

process.exitCode = main();
