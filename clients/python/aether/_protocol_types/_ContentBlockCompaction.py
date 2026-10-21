# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ContentBlockCompaction(TypedDict, total=False):
    """
    A server-side compaction summary (Anthropic `compact-2026-01-12`). It
    must be replayed verbatim: the API drops everything before it.
    """
    content: str
    type: Literal["compaction"]
