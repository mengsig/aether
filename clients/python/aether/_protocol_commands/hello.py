# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
    DeviceProof,
)

def hello(*, client_name: str, client_version: str, protocol_version: int, device: DeviceProof | None = None, features: list[str] | None = None, token: str | None = None) -> dict[str, Any]:
    """The `hello` command."""
    frame: dict[str, Any] = {"cmd": "hello"}
    frame["client_name"] = client_name
    frame["client_version"] = client_version
    if device is not None:
        frame["device"] = device
    if features is not None:
        frame["features"] = features
    frame["protocol_version"] = protocol_version
    if token is not None:
        frame["token"] = token
    return frame
