# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
)

def confirm_pairing(*, accept: bool, id: str) -> dict[str, Any]:
    """Enrol or reject the device that claimed a pairing. Host only."""
    frame: dict[str, Any] = {"cmd": "confirm_pairing"}
    frame["accept"] = accept
    frame["id"] = id
    return frame
