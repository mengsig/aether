# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
)

def unsubscribe(*, id: int) -> dict[str, Any]:
    """The `unsubscribe` command."""
    frame: dict[str, Any] = {"cmd": "unsubscribe"}
    frame["id"] = id
    return frame

def unsubscribe_push() -> dict[str, Any]:
    """Forget the calling device's push subscription."""
    frame: dict[str, Any] = {"cmd": "unsubscribe_push"}
    return frame
