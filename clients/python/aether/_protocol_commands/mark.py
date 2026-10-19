# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
    NotificationId,
)

def mark_notification_read(*, id: NotificationId) -> dict[str, Any]:
    """Mark a notification seen. Needs `features::NOTIFICATIONS`."""
    frame: dict[str, Any] = {"cmd": "mark_notification_read"}
    frame["id"] = id
    return frame
