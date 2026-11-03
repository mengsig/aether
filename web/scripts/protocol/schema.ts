// Reading a schemars JSON Schema (draft 2020-12) of a daemon protocol: the
// same rules as scripts/generate_clients.py, so both generators agree on shapes.

export type Schema = { readonly [key: string]: unknown };

export interface ProtocolSchema {
  readonly title: string;
  readonly protocolVersion: number;
  readonly minProtocolVersion: number | undefined;
  readonly defs: Readonly<Record<string, Schema>>;
  readonly clientFrame: string;
}

export interface Fields {
  readonly properties: Record<string, unknown>;
  readonly required: Set<string>;
}

export function isSchema(value: unknown): value is Schema {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function protocolInteger(document: Schema, key: string): number | undefined {
  const value = document[key];
  if (value === undefined) return undefined;
  if (!Number.isInteger(value) || (value as number) < 0) {
    throw new Error(`schema field ${key} must be a non-negative integer, not ${JSON.stringify(value)}`);
  }
  return value as number;
}

/** Validates the top level of a protocol schema and names its parts. */
export function parseProtocolSchema(document: unknown): ProtocolSchema {
  if (!isSchema(document)) throw new Error("a protocol schema must be a JSON object");
  const defs = document["$defs"];
  if (!isSchema(defs) || Object.keys(defs).length === 0) {
    throw new Error("a protocol schema needs a non-empty $defs object");
  }
  for (const [name, definition] of Object.entries(defs)) {
    if (!isSchema(definition)) throw new Error(`definition ${name} is not a schema object`);
  }
  const protocolVersion = protocolInteger(document, "protocol_version");
  if (protocolVersion === undefined) throw new Error("a protocol schema needs protocol_version");
  const minProtocolVersion = protocolInteger(document, "min_protocol_version");
  if (minProtocolVersion !== undefined && minProtocolVersion > protocolVersion) {
    throw new Error("min_protocol_version is above protocol_version");
  }
  const clientFrame = refName(document["client_frame"]);
  if (clientFrame === undefined || !(clientFrame in defs)) {
    throw new Error("client_frame must be a $ref to a definition in $defs");
  }
  const title = typeof document["title"] === "string" ? document["title"] : "daemon protocol";
  return { title, protocolVersion, minProtocolVersion, defs: defs as Record<string, Schema>, clientFrame };
}

/** The definition a `$ref` points at, or undefined. */
export function refName(schema: unknown): string | undefined {
  const ref = isSchema(schema) ? schema["$ref"] : undefined;
  if (typeof ref !== "string" || !ref.startsWith("#/$defs/")) return undefined;
  return ref.slice("#/$defs/".length);
}

function branchesOf(schema: Schema, key: "oneOf" | "anyOf"): readonly unknown[] | undefined {
  const branches = schema[key];
  return Array.isArray(branches) && branches.length > 0 ? branches : undefined;
}

/** The literal strings a schema accepts, or undefined if it is not a closed set of them. */
export function stringLiterals(schema: unknown): string[] | undefined {
  if (!isSchema(schema)) return undefined;
  if (schema["type"] === "string" && Array.isArray(schema["enum"])) {
    return schema["enum"].map((value) => String(value));
  }
  if (typeof schema["const"] === "string") return [schema["const"]];
  const branches = branchesOf(schema, "oneOf") ?? branchesOf(schema, "anyOf");
  if (branches === undefined) return undefined;
  const out: string[] = [];
  for (const branch of branches) {
    const literals = stringLiterals(branch);
    if (literals === undefined) return undefined;
    out.push(...literals);
  }
  return out;
}

/** [inner schema, nullable]: schemars writes `Option<T>` as `type: [T, "null"]` or an `anyOf` with null. */
export function splitNullable(schema: Schema): [Schema, boolean] {
  const kind = schema["type"];
  if (Array.isArray(kind) && kind.includes("null")) {
    const concrete = kind.filter((entry) => entry !== "null");
    if (concrete.length === 1) return [{ ...schema, type: concrete[0] }, true];
  }
  const branches = branchesOf(schema, "anyOf");
  if (branches === undefined) return [schema, false];
  const isNull = (branch: unknown) => isSchema(branch) && branch["type"] === "null" && Object.keys(branch).length === 1;
  const concrete = branches.filter((branch) => !isNull(branch));
  if (concrete.length === branches.length) return [schema, false];
  const [only] = concrete;
  if (concrete.length === 1 && isSchema(only)) return [only, true];
  return [{ anyOf: concrete }, true];
}

function constTag(branch: unknown, discriminant: string): string | undefined {
  if (!isSchema(branch) || !isSchema(branch["properties"])) return undefined;
  const field = branch["properties"][discriminant];
  return isSchema(field) && typeof field["const"] === "string" ? field["const"] : undefined;
}

/** The property every branch of a `oneOf` pins to a constant string: serde's tag. */
export function discriminantOf(schema: Schema): string | undefined {
  const branches = branchesOf(schema, "oneOf");
  if (branches === undefined || branches.length < 2) return undefined;
  let shared: Set<string> | undefined;
  for (const branch of branches) {
    if (!isSchema(branch) || !isSchema(branch["properties"])) return undefined;
    const tags = Object.keys(branch["properties"]).filter((name) => constTag(branch, name) !== undefined);
    shared = new Set(shared === undefined ? tags : tags.filter((name) => shared?.has(name)));
    if (shared.size === 0) return undefined;
  }
  return shared === undefined ? undefined : [...shared].sort()[0];
}

/** The branches of a union discriminated by `discriminant`, each with its tag. */
export function taggedUnion(schema: Schema, discriminant: string): Array<[string, Schema]> {
  const branches = branchesOf(schema, "oneOf") ?? [];
  return branches.map((branch, index) => {
    const tag = constTag(branch, discriminant);
    if (tag === undefined || !isSchema(branch)) {
      throw new Error(`union branch ${index} has no constant "${discriminant}"`);
    }
    return [tag, branch];
  });
}

function ownFields(schema: Schema | undefined): Fields {
  const properties = schema !== undefined && isSchema(schema["properties"]) ? { ...schema["properties"] } : {};
  const listed = schema?.["required"];
  const required = new Set(Array.isArray(listed) ? listed.filter((n): n is string => typeof n === "string") : []);
  return { properties, required };
}

/** A branch's fields: its own and those of the struct it flattens in by `$ref`, minus the tag. */
export function mergedFields(defs: Readonly<Record<string, Schema>>, variant: Schema, discriminant: string): Fields {
  const base = refName(variant);
  if (base !== undefined && !(base in defs)) throw new Error(`unknown $ref ${base}`);
  const inherited = ownFields(base === undefined ? undefined : defs[base]);
  const own = ownFields(variant);
  const merged = Object.entries({ ...inherited.properties, ...own.properties });
  const properties = Object.fromEntries(merged.filter(([name]) => name !== discriminant));
  const required = new Set([...inherited.required, ...own.required]);
  required.delete(discriminant);
  return { properties, required };
}

/** The doc text of a schema, one entry per source line, safe inside a block comment. */
export function describe(schema: unknown): string[] {
  const text = isSchema(schema) ? schema["description"] : undefined;
  if (typeof text !== "string" || text === "") return [];
  return text.split("\n").map((line) => line.trimEnd().replaceAll("*/", "*\\/"));
}
