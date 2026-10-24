# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class WriteStats(TypedDict, total=False):
    """
    Payload bytes the store committed since `since`, heaviest kind first.
    Counts JSON bodies, not SQLite pages: indexes and the WAL add on top, so
    the file sizes are reported next to it.
    """
    db_bytes: int | None  # optional
    kinds: list[_protocol.KindWrites]
    since: str | None  # optional
    total_bytes: int
    total_rows: int
    wal_bytes: int | None  # optional
