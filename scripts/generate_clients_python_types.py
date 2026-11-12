"""Emit protocol types with lazy field references and explicit alias dependencies."""

import ast


def type_files(source, banner):
    lines = source.splitlines()
    nodes = ast.parse(source).body
    constants = {"PROTOCOL_VERSION", "MIN_PROTOCOL_VERSION"}
    declarations = [node for node in nodes if isinstance(node, ast.ClassDef)
                    or isinstance(node, ast.Assign) and node.targets[0].id not in constants]
    names = {node.name if isinstance(node, ast.ClassDef) else node.targets[0].id
             for node in declarations}
    previous_end = next(node.end_lineno for node in nodes
                        if isinstance(node, ast.Assign)
                        and node.targets[0].id == "MIN_PROTOCOL_VERSION")
    root = lines[:previous_end] + ["", "from ._protocol_types import *", ""]
    header = [f"# {line}" for line in banner]
    output = {"_protocol_types/__init__.py": ""}
    exports = []
    index = list(header) + [""]
    for node in declarations:
        name = node.name if isinstance(node, ast.ClassDef) else node.targets[0].id
        body = lines[previous_end:node.end_lineno]
        imports = ["from typing import Any, Literal, TypedDict"]
        if isinstance(node, ast.ClassDef):
            # Field annotations are postponed. Referring through the public facade
            # supports recursive types without importing incomplete sibling classes.
            # get_type_hints resolves these after package initialization completes.
            imports.append("from .. import protocol as _protocol")
            replacements = [ref for field in node.body if isinstance(field, ast.AnnAssign)
                            for ref in ast.walk(field.annotation)
                            if isinstance(ref, ast.Name) and ref.id in names]
            for ref in sorted(replacements, key=lambda ref: (ref.lineno, ref.col_offset),
                              reverse=True):
                row = ref.lineno - previous_end - 1
                # AST columns are UTF-8 byte offsets, not Python character offsets.
                encoded = body[row].encode("utf-8")
                body[row] = (encoded[:ref.col_offset] + b"_protocol."
                             + encoded[ref.col_offset:]).decode("utf-8")
        else:
            dependencies = sorted({ref.id for ref in ast.walk(node.value)
                                   if isinstance(ref, ast.Name) and ref.id in names})
            imports.extend(f"from ._{dependency} import {dependency}"
                           for dependency in dependencies)
        rendered = "\n".join(header + ["from __future__ import annotations", ""]
                             + imports + [""] + body).rstrip() + "\n"
        output[f"_protocol_types/_{name}.py"] = rendered
        index.append(f"from ._{name} import {name}")
        exports.append(name)
        previous_end = node.end_lineno
    output.update(index_files(header, index[len(header) + 1:], exports))
    return output, root


# One import line per type is fixed, so the index is split in shards that
# each stay under 600 lines; twenty names a row keeps `__all__` short.
INDEX_SHARD = 400


def index_files(header, imports, exports):
    assert len(imports) == len(exports), "one import per exported type"
    output = {}
    shards = []
    for offset in range(0, len(exports), INDEX_SHARD):
        shard = f"_index_{len(shards)}"
        names = exports[offset:offset + INDEX_SHARD]
        lines = list(header) + [""] + imports[offset:offset + INDEX_SHARD]
        lines.extend(["", "__all__ = ["])
        for row in range(0, len(names), 20):
            lines.append("    " + ", ".join(repr(name) for name in names[row:row + 20]) + ",")
        lines.append("]")
        output[f"_protocol_types/{shard}.py"] = "\n".join(lines) + "\n"
        shards.append(shard)
    root = list(header) + [""]
    root.extend(f"from .{shard} import *" for shard in shards)
    root.append(f"from . import {', '.join(shards)}")
    root.extend(["", "__all__ = ["] + [f"    *{shard}.__all__," for shard in shards] + ["]"])
    output["_protocol_types/__init__.py"] = "\n".join(root) + "\n"
    return output
