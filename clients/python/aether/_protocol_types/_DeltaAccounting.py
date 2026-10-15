# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class DeltaAccounting(TypedDict, total=False):
    """
    Absolute in-memory figures. Replace prior totals; never add these to durable usage.
    """
    context_tokens: int | None  # optional
    delta: Literal["accounting"]
    total: _protocol.Usage
    turn_usage: _protocol.Usage
