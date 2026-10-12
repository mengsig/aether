# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class DeviceIdentity(TypedDict, total=False):
    """
    The device a connection authenticated as, echoed in [`crate::Welcome`].
    """
    id: _protocol.DeviceId
    name: str
    role: _protocol.DeviceRole
