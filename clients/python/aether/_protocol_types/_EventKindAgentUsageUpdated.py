# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class EventKindAgentUsageUpdated(TypedDict, total=False):
    """
    Absolute billing totals, including corrections from a completed engine run.
    """
    billing: _protocol.Billing | None  # optional
    total: _protocol.Usage
    turn_usage: _protocol.Usage
    type: Literal["agent.usage_updated"]
