# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class WindowHeadroom(TypedDict, total=False):
    name: str
    remaining_percent: int
    resets_at: str | None  # optional
    resets_in_seconds: int | None  # optional
    window_minutes: int | None  # optional
