# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ReplyNoesisEvidence(TypedDict, total=False):
    """
    `next_before` is the cursor for the next page, absent on the last one.
    """
    data: dict[str, Any]
    reply: Literal["noesis_evidence"]
