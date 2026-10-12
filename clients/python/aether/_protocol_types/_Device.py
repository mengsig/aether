# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class Device(TypedDict, total=False):
    """
    One enrolled device as `aether remote devices` and a phone's settings list it.
    """
    connections: int  # optional
    created_at: str
    current: bool  # optional
    fingerprint: str | None  # optional
    id: _protocol.DeviceId
    last_seen_at: str | None  # optional
    name: str
    push: bool  # optional
    revoked_at: str | None  # optional
    role: _protocol.DeviceRole
