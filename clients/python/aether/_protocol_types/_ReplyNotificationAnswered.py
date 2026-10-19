# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ReplyNotificationAnswered(TypedDict, total=False):
    """
    The answered notification, and the posting agent whose inbox the
    answer reached; `None` for a service, which reads the event.
    """
    data: dict[str, Any]
    reply: Literal["notification_answered"]
