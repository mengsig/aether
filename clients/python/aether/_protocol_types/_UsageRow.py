# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class UsageRow(TypedDict, total=False):
    """
    One group's spend.
    """
    invocations: int
    key: str
    usage: _protocol.Usage
