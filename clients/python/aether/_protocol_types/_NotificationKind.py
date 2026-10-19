# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict


# What a notification asks of the operator. Questions and blockers wait on
# them; the rest inform.
NotificationKind = Literal["question", "blocker", "decision", "outcome", "info"]
