# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class QuotaWindow(TypedDict, total=False):
    """
    A subscription usage window as an engine reported it. Opaque JSON on the
    wire from the CLI; normalized here so every reader agrees.
    """
    resets_at: str | None  # optional
    used_percent: int
    window_minutes: int | None  # optional
