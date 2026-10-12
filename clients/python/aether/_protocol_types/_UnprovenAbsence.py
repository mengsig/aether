# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict


# Why an absent receipt does not prove its key was never executed.
UnprovenAbsence = Literal["no_horizon", "foreign_horizon", "horizon_ahead", "forgotten"]
