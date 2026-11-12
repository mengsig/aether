"""Lay out generated TypeScript definitions without changing their declarations."""

import re


def declarations(lines):
    """Keep each exported declaration together with its leading documentation."""
    starts = []
    for index, line in enumerate(lines):
        if not line.startswith("export "):
            continue
        start = index
        while start and not lines[start - 1].strip():
            start -= 1
        if start and lines[start - 1].strip() == "*/":
            start -= 1
            while start and lines[start].strip() != "/**":
                start -= 1
        starts.append(start)
    if not starts:
        raise ValueError("generated TypeScript has no exported declarations")
    starts[0] = 0
    return [lines[start:end] for start, end in zip(starts, starts[1:] + [len(lines)])]


def exported_names(lines):
    return re.findall(r"^export (?:interface|type|function) (\w+)", "\n".join(lines), re.M)


def files(schema, definitions, commands, banner):
    """Return paths relative to src, with protocol.ts retaining every export."""
    modules = {}
    for name, lines in definitions.items():
        if name == "Reply":
            for declaration in declarations(lines):
                modules.setdefault("protocol/types/Reply.ts", []).extend(declaration)
            continue
        if name != "EventKind":
            modules[f"protocol/types/{name}.ts"] = lines
            continue
        for declaration in declarations(lines):
            tag = re.search(r'"type": "([^".]+)', "\n".join(declaration))
            group = tag.group(1) if tag else "union"
            modules.setdefault(f"protocol/events/{group}.ts", []).extend(declaration)

    group = None
    for declaration in declarations(commands):
        text = "\n".join(declaration)
        if text.lstrip().startswith("/** Every command") or "export type Command =" in text:
            group = "union"
        else:
            tag = re.search(r'"cmd": "([^"_]+)', text)
            if tag:
                group = tag.group(1)
        if group is None:
            raise ValueError("command builder has no preceding command interface")
        modules.setdefault(f"protocol/commands/{group}.ts", []).extend(declaration)

    owners = {}
    for path, lines in modules.items():
        for name in exported_names(lines):
            if name in owners:
                raise ValueError(f"duplicate generated export: {name}")
            owners[name] = path

    output = {}
    header = [f"// {line}" for line in banner]
    for path, lines in sorted(modules.items()):
        text = "\n".join(lines)
        own = set(exported_names(lines))
        # All cross-module dependencies are erased types. Importing through
        # the public entry point also handles recursive protocol definitions.
        referenced = sorted(name for name in owners if name not in own
                            and re.search(r"\b" + name + r"\b", text))
        imports = (["import type {"] + [f"  {name}," for name in referenced]
                   + ['} from "../../protocol.ts";']) if referenced else []
        rendered = "\n".join(header + [""] + imports + [""] + lines).rstrip() + "\n"
        if len(rendered.splitlines()) > 600:
            raise ValueError(f"generated module exceeds 600 lines: {path}")
        output[path] = rendered

    root = header + ["", f"export const PROTOCOL_VERSION = {schema['protocol_version']};",
                     f"export const MIN_PROTOCOL_VERSION = {schema['min_protocol_version']};", ""]
    root.extend(f'export * from "./{path}";' for path in sorted(modules))
    output["protocol.ts"] = "\n".join(root) + "\n"
    return output
