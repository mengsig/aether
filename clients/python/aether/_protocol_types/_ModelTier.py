# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict


# A capability band within one provider, weakest first. The vocabulary is
# shared with Archon's catalog tiers: keep the names stable. `strongest` is
# the legacy name of `strong` and is still accepted when reading.
ModelTier = Literal["fast", "balanced", "strong", "frontier"]
