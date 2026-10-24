# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class KindWrites(TypedDict, total=False):
    """
    Rows and payload bytes written for one kind of record: an event type such
    as `agent.usage_updated`, or a table such as `table:agents`.
    """
    bytes: int
    kind: str
    rows: int
