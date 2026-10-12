# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class RemoteStatus(TypedDict, total=False):
    """
    Reply to `get_remote` and `configure_remote`.
    """
    address: str
    allow_remote: bool  # optional
    connections: int  # optional
    devices: int  # optional
    error: str | None  # optional
    listening: str | None  # optional
    push_previews: bool  # optional
    revision: int
    settings: _protocol.RemoteSettings
