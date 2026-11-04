// Rendering a parsed protocol schema as one TypeScript module: a type per
// definition, then an interface and a builder per command of the client frame.

import {
  describe,
  discriminantOf,
  isSchema,
  mergedFields,
  refName,
  splitNullable,
  stringLiterals,
  taggedUnion,
  type Fields,
  type ProtocolSchema,
  type Schema,
} from "./schema.ts";

const PRIMITIVES: Readonly<Record<string, string>> = {
  string: "string",
  integer: "number",
  number: "number",
  boolean: "boolean",
};

// Words a generated builder name can never be.
const RESERVED = new Set([
  "break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete", "do",
  "else", "enum", "export", "extends", "false", "finally", "for", "function", "if", "import", "in",
  "instanceof", "new", "null", "return", "super", "switch", "this", "throw", "true", "try",
  "typeof", "var", "void", "while", "with", "yield", "let", "static", "implements", "interface",
  "package", "private", "protected", "public", "await",
]);

function literalUnion(values: readonly string[]): string {
  return values.map((value) => JSON.stringify(value)).join(" | ");
}

function unique(parts: readonly string[]): string[] {
  return [...new Set(parts)];
}

/** A type expression for one property schema. */
export function typeOf(schema: unknown): string {
  if (!isSchema(schema) || Object.keys(schema).length === 0) return "unknown";
  const named = refName(schema);
  if (named !== undefined) return named;
  const [inner, nullable] = splitNullable(schema);
  if (nullable) return `${typeOf(inner)} | null`;
  const literals = stringLiterals(schema);
  if (literals !== undefined && literals.length > 0) return literalUnion(literals);
  const declared = schema["type"];
  const kind = Array.isArray(declared) ? declared.find((entry) => entry !== "null") : declared;
  if (kind === "array") return `Array<${typeOf(schema["items"] ?? true)}>`;
  if (kind === "object") {
    const extra = schema["additionalProperties"];
    if (isSchema(schema["properties"]) || extra === undefined) return "Record<string, unknown>";
    return `Record<string, ${typeOf(extra)}>`;
  }
  if (typeof kind === "string" && kind in PRIMITIVES) return PRIMITIVES[kind] as string;
  const branches = schema["anyOf"] ?? schema["oneOf"];
  if (Array.isArray(branches) && branches.length > 0) return unique(branches.map(typeOf)).join(" | ");
  return "unknown";
}

function pascal(name: string): string {
  return name
    .split(/[^0-9a-zA-Z]+/)
    .filter((part) => part !== "")
    .map((part) => part[0]?.toUpperCase() + part.slice(1).toLowerCase())
    .join("");
}

function camel(name: string): string {
  const joined = pascal(name);
  return (joined[0]?.toLowerCase() ?? "") + joined.slice(1);
}

function documented(doc: readonly string[]): string[] {
  if (doc.length === 0) return [];
  return ["/**", ...doc.map((line) => ` * ${line}`.trimEnd()), " */"];
}

function interfaceLines(name: string, fields: Fields, doc: readonly string[], fixed: readonly string[] = []): string[] {
  const lines = [...documented(doc), `export interface ${name} {`, ...fixed];
  for (const field of Object.keys(fields.properties).sort()) {
    const mark = fields.required.has(field) ? "" : "?";
    lines.push(`  ${JSON.stringify(field)}${mark}: ${typeOf(fields.properties[field])};`);
  }
  return [...lines, "}", ""];
}

function unionLines(name: string, members: readonly string[]): string[] {
  const single = `export type ${name} = ${members.join(" | ")};`;
  if (single.length <= 100) return [single, ""];
  return [`export type ${name} =`, ...members.map((member, index) =>
    `  | ${member}${index === members.length - 1 ? ";" : ""}`), ""];
}

// A variant named like a definition (Archon's Request + status = RequestStatus) takes a Variant suffix.
function memberName(union: string, tag: string, defs: ProtocolSchema["defs"]): string {
  const plain = union + pascal(tag);
  return plain in defs ? `${plain}Variant` : plain;
}

function variantLines(name: string, schema: Schema, discriminant: string, defs: ProtocolSchema["defs"]): string[] {
  const lines = documented([...describe(schema), `Tagged by \`${discriminant}\`.`]);
  const members: string[] = [];
  for (const [tag, variant] of taggedUnion(schema, discriminant)) {
    const member = memberName(name, tag, defs);
    members.push(member);
    const fields = mergedFields(defs, variant, discriminant);
    fields.properties[discriminant] = { const: tag };
    fields.required.add(discriminant);
    lines.push(...interfaceLines(member, fields, describe(variant)));
  }
  return [...lines, ...unionLines(name, members)];
}

// The client frame is its envelope fields (the request id) joined to one command.
function clientFrameLines(name: string, schema: Schema, protocol: ProtocolSchema): string[] {
  const fields = mergedFields(protocol.defs, schema, "");
  const envelope = Object.keys(fields.properties).sort().map((field) =>
    `${JSON.stringify(field)}${fields.required.has(field) ? "" : "?"}: ${typeOf(fields.properties[field])}`);
  const joined = envelope.length === 0 ? "Command" : `{ ${envelope.join("; ")} } & Command`;
  return [...documented(describe(schema)), `export type ${name} = ${joined};`, ""];
}

/** The declarations of one `$defs` entry. */
export function definitionLines(name: string, schema: Schema, protocol: ProtocolSchema): string[] {
  const defs = protocol.defs;
  if (name === protocol.clientFrame) return clientFrameLines(name, schema, protocol);
  const doc = describe(schema);
  const literals = stringLiterals(schema);
  if (literals !== undefined && literals.length > 0) {
    return [...documented(doc), `export type ${name} = ${literalUnion(literals)};`, ""];
  }
  if (schema["type"] === "object" && isSchema(schema["properties"])) {
    return interfaceLines(name, mergedFields(defs, schema, ""), doc);
  }
  const discriminant = discriminantOf(schema);
  if (discriminant !== undefined) return variantLines(name, schema, discriminant, defs);
  if (schema["type"] === "string" && typeof schema["pattern"] === "string") {
    return [...documented([...doc, `Pattern: ${schema["pattern"]}`]), `export type ${name} = string;`, ""];
  }
  return [...documented(doc), `export type ${name} = ${typeOf(schema)};`, ""];
}

function commandLines(tag: string, variant: Schema, discriminant: string, protocol: ProtocolSchema): string[] {
  const interfaceName = `${pascal(tag)}Command`;
  const builder = camel(tag);
  if (RESERVED.has(builder) || !/^[a-z][0-9a-zA-Z]*$/.test(builder)) {
    throw new Error(`command ${JSON.stringify(tag)} gives an unusable builder name ${JSON.stringify(builder)}`);
  }
  const fields = mergedFields(protocol.defs, variant, discriminant);
  const tagLine = `  ${JSON.stringify(discriminant)}: ${JSON.stringify(tag)};`;
  const lines = interfaceLines(interfaceName, fields, describe(variant), [tagLine]);
  const frame = `${discriminant}: ${JSON.stringify(tag)}`;
  if (Object.keys(fields.properties).length === 0) {
    lines.push(`export function ${builder}(): ${interfaceName} {`, `  return { ${frame} };`);
  } else {
    lines.push(`export function ${builder}(args: Omit<${interfaceName}, ${JSON.stringify(discriminant)}>): ${interfaceName} {`,
      `  return { ${frame}, ...args };`);
  }
  return [...lines, "}", ""];
}

/** One interface and builder per client-frame command, then the `Command` union. */
export function commandsLines(protocol: ProtocolSchema): string[] {
  const frame = protocol.defs[protocol.clientFrame] as Schema;
  const discriminant = discriminantOf(frame);
  if (discriminant === undefined) {
    throw new Error(`${protocol.clientFrame} is not a tagged union, so it has no commands to build`);
  }
  const commands = taggedUnion(frame, discriminant).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  const lines = ["// Commands. Each builds one frame; the client adds the request id.", ""];
  for (const [tag, variant] of commands) lines.push(...commandLines(tag, variant, discriminant, protocol));
  const names = commands.map(([tag]) => `${pascal(tag)}Command`);
  return [...lines, "/** Every command the daemon accepts. */", ...unionLines("Command", names)];
}

function exportedNames(lines: readonly string[]): string[] {
  const names: string[] = [];
  for (const line of lines) {
    const match = /^export (?:interface|type|function|const) (\w+)/.exec(line);
    if (match?.[1] !== undefined) names.push(match[1]);
  }
  return names;
}

/** The whole generated module for one schema; `source` names the schema in the banner. */
export function renderModule(protocol: ProtocolSchema, source: string, sha256: string): string {
  if (!/^[0-9a-f]{64}$/.test(sha256)) throw new Error("the schema digest must be a SHA-256 hex string");
  const lines = [
    `// The ${protocol.title} as TypeScript types and command builders.`,
    `// Generated from ${source} by web/scripts/generate-protocol.ts.`,
    "// Do not edit: regenerate with `npm run generate` in web/.",
    `// Schema SHA-256: ${sha256}`,
    "",
    `export const PROTOCOL_VERSION = ${protocol.protocolVersion};`,
  ];
  if (protocol.minProtocolVersion !== undefined) {
    lines.push(`export const MIN_PROTOCOL_VERSION = ${protocol.minProtocolVersion};`);
  }
  lines.push("");
  for (const name of Object.keys(protocol.defs).sort()) {
    lines.push(...definitionLines(name, protocol.defs[name] as Schema, protocol));
  }
  lines.push(...commandsLines(protocol));
  const names = exportedNames(lines);
  const duplicate = names.find((name, index) => names.indexOf(name) !== index);
  if (duplicate !== undefined) throw new Error(`two generated declarations are named ${duplicate}`);
  return lines.join("\n").trimEnd() + "\n";
}
