#!/usr/bin/env python3
"""Generate the Python and TypeScript protocol libraries from the schema.

The Rust types are the source. `aether schema` prints them as JSON Schema,
`clients/protocol.schema.json` is that document committed, and this script
turns it into `clients/python/aether/protocol.py` and
`clients/typescript/src/protocol.ts`. The runtimes beside those files
(`client.py`, `client.ts`) are written by hand and are not touched here.

    scripts/generate_clients.py                 # write the libraries
    scripts/generate_clients.py --check         # fail if they are stale
    scripts/generate_clients.py --binary path/to/aether   # refresh the schema first

Output is deterministic: the same schema always produces the same bytes, so
`--check` is a drift gate and not a formatting argument.
"""

import argparse
import json
import keyword
import pathlib
import re
import subprocess
import sys
if __package__:
    from .generate_clients_python import files as python_files
    from .generate_clients_typescript import files as typescript_files
else:
    from generate_clients_python import files as python_files
    from generate_clients_typescript import files as typescript_files

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCHEMA_PATH = ROOT / "clients" / "protocol.schema.json"
PYTHON_OUT = ROOT / "clients" / "python" / "aether" / "protocol.py"
TS_OUT = ROOT / "clients" / "typescript" / "src" / "protocol.ts"

BANNER = "Generated from clients/protocol.schema.json by scripts/generate_clients.py."
DO_NOT_EDIT = "Do not edit: change the Rust types and regenerate."


# --------------------------------------------------------------------------
# Reading the schema
# --------------------------------------------------------------------------


def load(path):
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)


def ref_name(schema):
    """The definition a `$ref` points at, or None."""
    ref = schema.get("$ref") if isinstance(schema, dict) else None
    if not isinstance(ref, str) or not ref.startswith("#/$defs/"):
        return None
    return ref[len("#/$defs/") :]


def string_literals(schema):
    """The literal strings a schema accepts, or None if it is not a closed
    set of them. Covers both `enum` and a `oneOf` of `const`s."""
    if not isinstance(schema, dict):
        return None
    if schema.get("type") == "string" and isinstance(schema.get("enum"), list):
        return [str(value) for value in schema["enum"]]
    if isinstance(schema.get("const"), str):
        return [schema["const"]]
    branches = schema.get("oneOf") or schema.get("anyOf")
    if not isinstance(branches, list) or not branches:
        return None
    out = []
    for branch in branches:
        literals = string_literals(branch)
        if literals is None:
            return None
        out.extend(literals)
    return out


def split_nullable(schema):
    """(inner schema, nullable). A schemars `Option<T>` is either an `anyOf`
    of T and null (T a $ref or a union), or - for a primitive T like
    `Option<u64>` - a single schema whose `"type"` is `[T, "null"]`."""
    if not isinstance(schema, dict):
        return schema, False
    kind = schema.get("type")
    if isinstance(kind, list) and "null" in kind:
        concrete_kinds = [k for k in kind if k != "null"]
        if len(concrete_kinds) == 1:
            return {**schema, "type": concrete_kinds[0]}, True
    branches = schema.get("anyOf")
    if not isinstance(branches, list):
        return schema, False
    concrete = [b for b in branches if b != {"type": "null"}]
    if len(concrete) == len(branches):
        return schema, False
    if len(concrete) == 1:
        return concrete[0], True
    return {"anyOf": concrete}, True


def variant_tag(variant, discriminant):
    """The constant this union branch carries in `discriminant`."""
    tag = variant.get("properties", {}).get(discriminant, {}).get("const")
    return tag if isinstance(tag, str) else None


def tagged_union(schema, discriminant):
    """The branches of a union discriminated by a constant property, or None."""
    branches = schema.get("oneOf") if isinstance(schema, dict) else None
    if not isinstance(branches, list) or not branches:
        return None
    tagged = [(variant_tag(b, discriminant), b) for b in branches]
    if any(tag is None for tag, _ in tagged):
        return None
    return tagged


def discriminant_of(schema):
    """The property every branch of this union pins to a constant string -
    serde's tag. `None` when the union is not tagged that way."""
    branches = schema.get("oneOf") if isinstance(schema, dict) else None
    if not isinstance(branches, list) or len(branches) < 2:
        return None
    shared = None
    for branch in branches:
        if not isinstance(branch, dict):
            return None
        tags = {
            name
            for name, field in branch.get("properties", {}).items()
            if isinstance(field, dict) and isinstance(field.get("const"), str)
        }
        shared = tags if shared is None else shared & tags
        if not shared:
            return None
    return sorted(shared)[0]


def merged_fields(defs, variant, discriminant):
    """A union branch's fields: its own, plus those of the struct it flattens
    in through `$ref`, with the discriminant removed."""
    properties = {}
    required = set()
    base = ref_name(variant)
    if base is not None:
        target = defs[base]
        properties.update(target.get("properties", {}))
        required.update(target.get("required", []))
    properties.update(variant.get("properties", {}))
    required.update(variant.get("required", []))
    properties.pop(discriminant, None)
    required.discard(discriminant)
    return properties, required


def variant_fields(defs, variant, discriminant, tag):
    """A union member's fields as a type declares them: the branch's own and
    its `$ref` struct's, plus the discriminant pinned to this member's tag."""
    properties, required = merged_fields(defs, variant, discriminant)
    properties[discriminant] = {"const": tag}
    if discriminant in variant.get("required", []):
        required.add(discriminant)
    return properties, required


def describe(schema):
    """The doc text for a schema, as one line per source line."""
    text = schema.get("description") if isinstance(schema, dict) else None
    return [line.rstrip() for line in text.splitlines()] if text else []


def identifier(name, reserved):
    """A field name usable as an identifier in the target language."""
    return f"{name}_" if name in reserved else name


# --------------------------------------------------------------------------
# Type expressions
# --------------------------------------------------------------------------


class Lang:
    """Everything that differs between the two targets, in one place."""

    def __init__(self, any_type, mapping, optional, list_of, dict_of, union, literal, reserved):
        self.any_type = any_type
        self.mapping = mapping
        self.optional = optional
        self.list_of = list_of
        self.dict_of = dict_of
        self.union = union
        self.literal = literal
        self.reserved = reserved


PYTHON = Lang(
    any_type="Any",
    mapping={"string": "str", "integer": "int", "number": "float", "boolean": "bool"},
    optional=lambda inner: f"{inner} | None",
    list_of=lambda inner: f"list[{inner}]",
    dict_of=lambda inner: f"dict[str, {inner}]",
    union=lambda parts: " | ".join(parts),
    literal=lambda values: "Literal[" + ", ".join(json.dumps(v) for v in values) + "]",
    reserved=set(keyword.kwlist),
)

TYPESCRIPT = Lang(
    any_type="unknown",
    mapping={"string": "string", "integer": "number", "number": "number", "boolean": "boolean"},
    optional=lambda inner: f"{inner} | null",
    list_of=lambda inner: f"Array<{inner}>",
    dict_of=lambda inner: f"Record<string, {inner}>",
    union=lambda parts: " | ".join(parts),
    literal=lambda values: " | ".join(json.dumps(v) for v in values),
    reserved=set(),
)


def dedupe(parts):
    """Union members, each kept once, in the order they were written."""
    seen = []
    for part in parts:
        if part not in seen:
            seen.append(part)
    return seen


def type_of(lang, schema):
    """A type expression for one property schema."""
    if schema is True or schema == {}:
        return lang.any_type
    if not isinstance(schema, dict):
        return lang.any_type
    named = ref_name(schema)
    if named is not None:
        return named
    inner, nullable = split_nullable(schema)
    if nullable:
        return lang.optional(type_of(lang, inner))
    literals = string_literals(schema)
    if literals:
        return lang.literal(literals)
    kind = schema.get("type")
    if isinstance(kind, list):
        kind = next((k for k in kind if k != "null"), None)
    if kind == "array":
        return lang.list_of(type_of(lang, schema.get("items", True)))
    if kind == "object":
        extra = schema.get("additionalProperties")
        if schema.get("properties"):
            return lang.dict_of(lang.any_type)
        return lang.dict_of(type_of(lang, extra) if extra is not None else lang.any_type)
    if kind in lang.mapping:
        return lang.mapping[kind]
    branches = schema.get("anyOf") or schema.get("oneOf")
    if isinstance(branches, list) and branches:
        return lang.union(dedupe([type_of(lang, branch) for branch in branches]))
    return lang.any_type


def snake_to_camel(name):
    head, *rest = name.split("_")
    return head + "".join(part.title() for part in rest)


def snake_to_pascal(name):
    return "".join(part.title() for part in name.split("_"))


def member_name(union, tag):
    """A type name for one variant. Serde tags carry dots (`agent.created`),
    which no target language accepts in an identifier."""
    return union + snake_to_pascal(re.sub(r"[^0-9a-zA-Z]+", "_", tag))


def wrapped_union(prefix, members, joiner):
    """`prefix` and the union, on one line where it fits."""
    single = prefix + joiner.join(members)
    if len(single) <= 95:
        return [single + ";" if joiner == " | " and prefix.startswith("export") else single]
    lines = [prefix.rstrip() + " ("] if not prefix.startswith("export") else [prefix.rstrip()]
    for index, member in enumerate(members):
        tail = "" if index == len(members) - 1 else joiner.strip()
        lines.append(f"    {member} {tail}".rstrip())
    lines.append(")" if not prefix.startswith("export") else ";")
    return lines


# --------------------------------------------------------------------------
# Python
# --------------------------------------------------------------------------


def python_library(schema):
    defs = schema["$defs"]
    out = [
        '"""The Aether daemon protocol as Python types and command builders.',
        "",
        BANNER,
        DO_NOT_EDIT,
        '"""',
        "",
        "from __future__ import annotations",
        "",
        "from typing import Any, Literal, TypedDict",
        "",
        f"PROTOCOL_VERSION = {schema['protocol_version']}",
        f"MIN_PROTOCOL_VERSION = {schema['min_protocol_version']}",
        "",
    ]
    for name in sorted(defs):
        out.extend(python_definition(name, defs[name], defs))
    out.extend(python_commands(defs))
    return "\n".join(out).rstrip() + "\n"


def python_comment(doc):
    return [f"# {line}".rstrip() for line in doc]


def python_class(name, properties, required, doc):
    """One TypedDict per definition.

    Every class is `total=False` and required-ness survives as a `# optional`
    marker on the fields that are not required, rather than as types a checker
    enforces. `typing.Required` would enforce it, and it is 3.11+; the command
    builders - which is where a caller actually constructs a frame - do take
    required-ness as keyword-only arguments, so the checkable surface is the
    one that matters. `clients/python/README.md` says so too.
    """
    lines = [f"class {name}(TypedDict, total=False):"]
    if doc:
        lines.append('    """')
        lines.extend(f"    {line}".rstrip() for line in doc)
        lines.append('    """')
    if not properties:
        lines.append("    pass")
    for field in sorted(properties):
        hint = type_of(PYTHON, properties[field])
        note = "" if field in required else "  # optional"
        lines.append(f"    {identifier(field, PYTHON.reserved)}: {hint}{note}")
    return lines + [""]


def python_definition(name, schema, defs):
    doc = describe(schema)
    literals = string_literals(schema)
    if literals:
        return python_comment(doc) + [f"{name} = {PYTHON.literal(literals)}", ""]
    properties = schema.get("properties")
    if schema.get("type") == "object" and properties:
        return python_class(name, properties, set(schema.get("required", [])), doc)
    discriminant = discriminant_of(schema)
    if discriminant:
        return python_variants(name, schema, discriminant, doc, defs)
    if schema.get("type") == "string" and schema.get("pattern"):
        return python_comment(doc + [f"Pattern: {schema['pattern']}"]) + [f"{name} = str", ""]
    return python_comment(doc) + [f"{name} = {type_of(PYTHON, schema)}", ""]


def python_variants(name, schema, discriminant, doc, defs):
    """A serde-tagged union: one class per variant, then the alias."""
    lines = python_comment(doc + [f"Tagged by `{discriminant}`."])
    members = []
    for tag, variant in tagged_union(schema, discriminant):
        member = member_name(name, tag)
        members.append(member)
        properties, required = variant_fields(defs, variant, discriminant, tag)
        lines.extend(python_class(member, properties, required, describe(variant)))
    lines.extend(wrapped_union(f"{name} = ", members, " | "))
    return lines + [""]


def python_commands(defs):
    branches = tagged_union(defs["ClientFrame"], "cmd")
    out = [
        "",
        "# --------------------------------------------------------------------------",
        "# Commands. Each builds one frame; the client adds the request id.",
        "# --------------------------------------------------------------------------",
        "",
    ]
    for tag, variant in sorted(branches, key=lambda pair: pair[0]):
        properties, required = merged_fields(defs, variant, "cmd")
        params = []
        for field in sorted(properties, key=lambda f: (f not in required, f)):
            hint = type_of(PYTHON, properties[field])
            arg = identifier(field, PYTHON.reserved)
            if field in required:
                params.append(f"{arg}: {hint}")
            else:
                optional = hint if hint.endswith("| None") else PYTHON.optional(hint)
                params.append(f"{arg}: {optional} = None")
        signature = ", ".join(["*"] + params) if params else ""
        out.append(f"def {identifier(tag, PYTHON.reserved)}({signature}) -> dict[str, Any]:")
        doc = describe(variant) or describe(defs.get(ref_name(variant) or "", {}))
        doc = doc or [f"The `{tag}` command."]
        if len(doc) == 1:
            out.append(f'    """{doc[0]}"""')
        else:
            # Every source line joins the docstring; a bare `#` comment
            # after a one-line `"""..."""` used to swallow the rest.
            out.append(f'    """{doc[0]}')
            out.extend(f"    {line}".rstrip() for line in doc[1:])
            out.append('    """')
        out.append(f'    frame: dict[str, Any] = {{"cmd": {json.dumps(tag)}}}')
        for field in sorted(properties):
            arg = identifier(field, PYTHON.reserved)
            if field in required:
                out.append(f"    frame[{json.dumps(field)}] = {arg}")
            else:
                out.append(f"    if {arg} is not None:")
                out.append(f"        frame[{json.dumps(field)}] = {arg}")
        out.append("    return frame")
        out.append("")
    return out


# --------------------------------------------------------------------------
# TypeScript
# --------------------------------------------------------------------------


def typescript_library(schema):
    defs = schema["$defs"]
    out = [
        "// The Aether daemon protocol as TypeScript types and command builders.",
        "//",
        f"// {BANNER}",
        f"// {DO_NOT_EDIT}",
        "",
        f"export const PROTOCOL_VERSION = {schema['protocol_version']};",
        f"export const MIN_PROTOCOL_VERSION = {schema['min_protocol_version']};",
        "",
    ]
    for name in sorted(defs):
        out.extend(typescript_definition(name, defs[name], defs))
    out.extend(typescript_commands(defs))
    return "\n".join(out).rstrip() + "\n"


def typescript_documented(doc):
    if not doc:
        return []
    return ["/**"] + [f" * {line}".rstrip() for line in doc] + [" */"]


def typescript_interface(name, properties, required, doc):
    lines = typescript_documented(doc) + [f"export interface {name} {{"]
    for field in sorted(properties):
        hint = type_of(TYPESCRIPT, properties[field])
        mark = "" if field in required else "?"
        lines.append(f"  {json.dumps(field)}{mark}: {hint};")
    return lines + ["}", ""]


def typescript_definition(name, schema, defs):
    doc = describe(schema)
    literals = string_literals(schema)
    if literals:
        return typescript_documented(doc) + [
            f"export type {name} = {TYPESCRIPT.literal(literals)};",
            "",
        ]
    properties = schema.get("properties")
    if schema.get("type") == "object" and properties:
        return typescript_interface(name, properties, set(schema.get("required", [])), doc)
    discriminant = discriminant_of(schema)
    if discriminant:
        return typescript_variants(name, schema, discriminant, doc, defs)
    if schema.get("type") == "string" and schema.get("pattern"):
        return typescript_documented(doc + [f"Pattern: {schema['pattern']}"]) + [
            f"export type {name} = string;",
            "",
        ]
    return typescript_documented(doc) + [
        f"export type {name} = {type_of(TYPESCRIPT, schema)};",
        "",
    ]


def typescript_variants(name, schema, discriminant, doc, defs):
    lines = typescript_documented(doc + [f"Tagged by `{discriminant}`."])
    members = []
    for tag, variant in tagged_union(schema, discriminant):
        member = member_name(name, tag)
        members.append(member)
        properties, required = variant_fields(defs, variant, discriminant, tag)
        lines.extend(typescript_interface(member, properties, required, describe(variant)))
    lines.extend(wrapped_union(f"export type {name} = ", members, " | "))
    return lines + [""]


def typescript_commands(defs):
    branches = tagged_union(defs["ClientFrame"], "cmd")
    out = [
        "",
        "// Commands. Each builds one frame; the client adds the request id.",
        "",
    ]
    names = []
    for tag, variant in sorted(branches, key=lambda pair: pair[0]):
        properties, required = merged_fields(defs, variant, "cmd")
        interface = f"{snake_to_pascal(tag)}Command"
        names.append(interface)
        out.extend(typescript_documented(describe(variant)))
        out.append(f"export interface {interface} {{")
        out.append(f"  \"cmd\": {json.dumps(tag)};")
        for field in sorted(properties):
            hint = type_of(TYPESCRIPT, properties[field])
            mark = "" if field in required else "?"
            out.append(f"  {json.dumps(field)}{mark}: {hint};")
        out.append("}")
        out.append("")
        argument = f"Omit<{interface}, \"cmd\">"
        builder = snake_to_camel(tag)
        if properties:
            out.append(f"export function {builder}(args: {argument}): {interface} {{")
            out.append(f"  return {{ cmd: {json.dumps(tag)}, ...args }};")
        else:
            out.append(f"export function {builder}(): {interface} {{")
            out.append(f"  return {{ cmd: {json.dumps(tag)} }};")
        out.append("}")
        out.append("")
    out.append("/** Every command the daemon accepts. */")
    out.append("export type Command =")
    out.extend([f"  | {name}" for name in names[:-1]])
    out.append(f"  | {names[-1]};")
    out.append("")
    return out


# --------------------------------------------------------------------------
# Entry point
# --------------------------------------------------------------------------


def refresh_schema(binary):
    """Rewrite the committed schema from the binary's own types."""
    printed = subprocess.run([binary, "schema"], check=True, capture_output=True, text=True)
    SCHEMA_PATH.write_text(printed.stdout, encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="fail if a file is stale")
    parser.add_argument("--binary", help="regenerate the schema with this aether binary first")
    args = parser.parse_args()

    if args.binary:
        if args.check:
            printed = subprocess.run(
                [args.binary, "schema"], check=True, capture_output=True, text=True
            )
            if printed.stdout != SCHEMA_PATH.read_text(encoding="utf-8"):
                print(
                    f"{SCHEMA_PATH.relative_to(ROOT)} is stale: the Rust protocol types changed.\n"
                    "Run scripts/generate_clients.py --binary <aether> to update it.",
                    file=sys.stderr,
                )
                return 1
        else:
            refresh_schema(args.binary)

    schema = load(SCHEMA_PATH)
    generated_py = python_files(python_library(schema), [BANNER, DO_NOT_EDIT])
    wanted = {PYTHON_OUT.parent / path: text for path, text in generated_py.items()}
    generated_ts = typescript_files(
        schema,
        {
            name: typescript_definition(name, value, schema["$defs"])
            for name, value in sorted(schema["$defs"].items())
        },
        typescript_commands(schema["$defs"]),
        [BANNER, DO_NOT_EDIT],
    )
    wanted.update({TS_OUT.parent / path: text for path, text in generated_ts.items()})
    obsolete = set((TS_OUT.parent / "protocol").rglob("*.ts")) - wanted.keys()
    obsolete |= set((PYTHON_OUT.parent / "_protocol_commands").glob("*.py")) - wanted.keys()
    obsolete |= set((PYTHON_OUT.parent / "_protocol_types").glob("*.py")) - wanted.keys()
    if obsolete:
        for path in sorted(obsolete):
            print(f"Unexpected protocol module; review before removing: {path}", file=sys.stderr)
        return 1
    stale = []
    for path, text in wanted.items():
        if args.check:
            current = path.read_text(encoding="utf-8") if path.exists() else None
            if current != text:
                stale.append(path)
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding="utf-8")
    if stale:
        for path in stale:
            print(f"{path.relative_to(ROOT)} is stale", file=sys.stderr)
        print("Run scripts/generate_clients.py to update the client libraries.", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
