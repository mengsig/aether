# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict


# Ordered weakest first, so the stronger of two is their maximum.
Verification = Literal["inferred", "observed", "verified"]
