# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict


# What a paired device may do. See the authorization matrix in docs/remote.md.
DeviceRole = Literal["operator", "observer"]
