# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class Pairing(TypedDict, total=False):
    """
    One pairing attempt. Reply to the four pairing commands.
    """
    candidate: _protocol.PairingCandidate | None  # optional
    code: str | None  # optional
    device: _protocol.DeviceIdentity | None  # optional
    expires_at: str
    id: str
    role: _protocol.DeviceRole
    state: _protocol.PairingState
    url: str | None  # optional
