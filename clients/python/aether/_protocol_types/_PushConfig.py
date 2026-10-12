# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class PushConfig(TypedDict, total=False):
    """
    Reply to the push commands: what the daemon sends this device.
    """
    enabled: bool
    kinds: list[_protocol.AttentionKind]  # optional
    previews: bool  # optional
    subscribed: bool  # optional
    vapid_public_key: str | None  # optional
