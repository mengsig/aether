# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
)

def refresh_provider_usage(*, provider: str | None = None) -> dict[str, Any]:
    """Refresh stale quota observations through explicitly enabled probe turns."""
    frame: dict[str, Any] = {"cmd": "refresh_provider_usage"}
    if provider is not None:
        frame["provider"] = provider
    return frame
