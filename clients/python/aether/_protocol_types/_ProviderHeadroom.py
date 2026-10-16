# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ProviderHeadroom(TypedDict, total=False):
    """
    Read-time projection of the latest report; unknown usage is always stale.
    """
    age_seconds: int | None  # optional
    observed_at: str | None  # optional
    stale: bool
    windows: list[_protocol.WindowHeadroom]
