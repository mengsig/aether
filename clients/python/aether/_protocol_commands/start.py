# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
    DeviceRole,
)

def start_pairing(*, role: DeviceRole | None = None) -> dict[str, Any]:
    """Open a pairing and mint its single-use code. Host only."""
    frame: dict[str, Any] = {"cmd": "start_pairing"}
    if role is not None:
        frame["role"] = role
    return frame
