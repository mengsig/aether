# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class Health(TypedDict, total=False):
    """
    Body of `GET /api/health`.
    """
    min_protocol_version: int
    pairing_open: bool
    protocol_version: int
    status: str
