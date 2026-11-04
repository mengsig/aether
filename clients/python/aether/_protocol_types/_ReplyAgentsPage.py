# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ReplyAgentsPage(TypedDict, total=False):
    """
    Reply to `list_agents_page`. `next_after` is the cursor for the next
    page, absent on the last one. Ids are only ordered, not committed in
    order, so a walk covers the agents present when it began; a client
    subscribes first and applies the buffered events for creations during it.
    """
    data: dict[str, Any]
    reply: Literal["agents_page"]
