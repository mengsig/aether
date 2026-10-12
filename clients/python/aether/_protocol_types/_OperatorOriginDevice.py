# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class OperatorOriginDevice(TypedDict, total=False):
    """
    A paired remote device, named as the daemon enrolled it.
    """
    connection: str
    device: _protocol.DeviceId
    name: str
    transport: Literal["device"]
