# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class Welcome(TypedDict, total=False):
    daemon_protocol_version: int | None  # optional
    daemon_version: str
    device: _protocol.DeviceIdentity | None  # optional
    features: list[str]  # optional
    idle_timeout_secs: int | None  # optional
    instance_id: str
    min_protocol_version: int | None  # optional
    pid: int
    protocol_version: int
    schema_version: int
    submission_horizon: str | None  # optional
