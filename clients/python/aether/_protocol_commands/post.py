# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
    NotificationPost,
)

def post_notification(*, notification: NotificationPost, source: str) -> dict[str, Any]:
    """Post a notification as the service `source`, or update the one its
    `key` already names. Needs `features::NOTIFICATIONS`.
    """
    frame: dict[str, Any] = {"cmd": "post_notification"}
    frame["notification"] = notification
    frame["source"] = source
    return frame
