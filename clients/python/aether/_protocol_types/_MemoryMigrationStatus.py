# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class MemoryMigrationStatus(TypedDict, total=False):
    backups: list[_protocol.MigrationBackup]  # optional
    last_import: _protocol.LastImport | None  # optional
    memories: _protocol.MemoryCounts
    migrations: list[_protocol.MemoryMigration]
    schema_version: int
