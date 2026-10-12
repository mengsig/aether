# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ServerFrameChallenge(TypedDict, total=False):
    """
    The remote carrier speaking first: what a device signs to
    authenticate its hello. Never sent on the socket or the token listener.
    """
    frame: Literal["challenge"]
    instance_id: str
    nonce: str
