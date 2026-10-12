# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class Hello(TypedDict, total=False):
    client_name: str
    client_version: str
    device: _protocol.DeviceProof | None  # optional
    features: list[str]  # optional
    protocol_version: int
    token: str | None  # optional
