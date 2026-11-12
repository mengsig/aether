# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class UsageBreakdown(TypedDict, total=False):
    """
    Reply to `usage_breakdown`: rows ordered by `key`, and the total over
    every invocation in scope, those of rows cut by `limit` included.
    """
    rows: list[_protocol.UsageRow]
    total: _protocol.Usage
    truncated: bool
