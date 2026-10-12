# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class DeviceProof(TypedDict, total=False):
    """
    A device's answer to the carrier's challenge, sent in [`crate::Hello`].
    """
    id: _protocol.DeviceId
    signature: str
