// The files of @aether-ui/protocol that come from the schemas, as a pure
// function of the schema texts: the same inputs always give the same bytes.

import { createHash } from "node:crypto";
import { renderModule } from "./render.ts";
import { parseProtocolSchema } from "./schema.ts";

export type ModuleName = "aether" | "archon";

export interface SchemaSource {
  readonly module: ModuleName;
  /** The schema's path from the repository root, shown in the banner. */
  readonly label: string;
  readonly text: string;
}

/** Every file name the generator may write into the package's `src/`. */
export const GENERATED_FILES: readonly string[] = ["aether.ts", "archon.ts", "index.ts"];

/** Marks a file the generator wrote, so it may replace or remove it. */
export const GENERATED_MARK = "by web/scripts/generate-protocol.ts";

function indexModule(modules: readonly ModuleName[]): string {
  const lines = [
    "// The @aether-ui/protocol entry point.",
    `// Generated ${GENERATED_MARK}: regenerate with \`npm run generate\` in web/.`,
    "",
    'export * from "./app.ts";',
    'export * from "./setup.ts";',
    ...modules.map((name) => `export * as ${name} from "./${name}.ts";`),
  ];
  return lines.join("\n") + "\n";
}

function parseJson(source: SchemaSource): unknown {
  try {
    return JSON.parse(source.text);
  } catch (error) {
    throw new Error(`${source.label} is not valid JSON`, { cause: error });
  }
}

/** File name to contents, for `src/` of @aether-ui/protocol. */
export function generateProtocol(sources: readonly SchemaSource[]): Map<string, string> {
  const modules = sources.map((source) => source.module);
  if (!modules.includes("aether")) throw new Error("the Aether schema is required");
  if (new Set(modules).size !== modules.length) throw new Error("each module needs exactly one schema");
  const files = new Map<string, string>();
  for (const source of [...sources].sort((a, b) => a.module.localeCompare(b.module))) {
    const sha256 = createHash("sha256").update(source.text, "utf8").digest("hex");
    try {
      const protocol = parseProtocolSchema(parseJson(source));
      files.set(`${source.module}.ts`, renderModule(protocol, source.label, sha256));
    } catch (error) {
      throw new Error(`cannot generate ${source.module}.ts from ${source.label}`, { cause: error });
    }
  }
  files.set("index.ts", indexModule([...modules].sort()));
  return files;
}
