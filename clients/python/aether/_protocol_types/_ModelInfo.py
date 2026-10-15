# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ModelInfo(TypedDict, total=False):
    aliases: list[str]  # optional
    auth_source: str | None  # optional
    capabilities: _protocol.CapabilityProfile
    configured: bool
    is_default: bool
    model: _protocol.ModelRef
    name: str
    provider_name: str
    tier: str | None  # optional
