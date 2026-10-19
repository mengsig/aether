# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
    NotificationId,
)

def answer_notification(*, id: NotificationId, action: str | None = None, text: str | None = None) -> dict[str, Any]:
    """Answer a notification with one of its actions, free text where it
    takes it, or both; this resolves it and reaches a posting agent's
    inbox. Needs `features::NOTIFICATIONS`.
    """
    frame: dict[str, Any] = {"cmd": "answer_notification"}
    if action is not None:
        frame["action"] = action
    frame["id"] = id
    if text is not None:
        frame["text"] = text
    return frame
