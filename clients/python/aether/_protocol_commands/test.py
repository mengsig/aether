# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
)

def test_push() -> dict[str, Any]:
    """Send the calling device one `test` notification."""
    frame: dict[str, Any] = {"cmd": "test_push"}
    return frame
