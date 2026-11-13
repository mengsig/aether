# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ReplyConversation(TypedDict, total=False):
    """
    Reply to `get_conversation`: the entries of the events between
    `from_seq` and `to_seq` (0 for an empty page), in order. Page on with
    `before: from_seq` or `after: to_seq`; `last_seq` is the agent's newest
    conversation event.
    """
    data: dict[str, Any]
    reply: Literal["conversation"]
