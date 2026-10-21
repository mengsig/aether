# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ContentBlockCompaction(TypedDict, total=False):
    """
    A server-side compaction summary, replayed verbatim. Signed summaries
    must lead the request, with the summarized history removed.
    """
    content: str
    signature: str | None  # optional
    type: Literal["compaction"]
