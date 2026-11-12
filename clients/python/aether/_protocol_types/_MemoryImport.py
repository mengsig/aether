# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class MemoryImport(TypedDict, total=False):
    conflicts: list[_protocol.ImportConflict]  # optional
    digest: str
    dry_run: bool
    inserted: int
    records: int
    unchanged: int
