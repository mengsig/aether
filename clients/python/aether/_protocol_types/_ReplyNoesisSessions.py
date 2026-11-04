# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ReplyNoesisSessions(TypedDict, total=False):
    """
    `next_after` is the cursor for the next page, absent on the last one.
    """
    data: dict[str, Any]
    reply: Literal["noesis_sessions"]
