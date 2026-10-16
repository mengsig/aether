# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ProviderInfo(TypedDict, total=False):
    """
    One provider as a client sees it: what it is, whether it can be used, and
    how it is behaving right now. `health` is live scheduler state, so a
    provider that has done nothing since the daemon started reads as idle.
    """
    auth_source: str | None  # optional
    base_url: str  # optional
    configured: bool
    enabled: bool
    engine: bool
    family: str | None  # optional
    headroom: _protocol.ProviderHeadroom  # optional
    health: _protocol.ProviderHealth
    models: int
    name: str
    open_model_list: bool
    protocol: str
    provider: str
